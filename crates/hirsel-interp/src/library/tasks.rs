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
    pub shielded: bool,
    spawn_counts: BTreeMap<u64, u32>,
    active_scopes: BTreeMap<u64, Vec<u64>>,
    scope_canceled: BTreeMap<u64, u32>,
    next: u64,
    starts: Vec<TaskStart>,
    tasks: BTreeMap<u64, Child>,
    scopes: BTreeMap<u64, ScopeState>,
    pub pending: BTreeMap<u64, Pending>,
}
#[derive(Default)]
struct ScopeState {
    children: Vec<u64>,
    // Completed detached failures still belong to the scope. Keep each child's
    // chain separately so completion order cannot change spawn-order reporting.
    failures: BTreeMap<Vec<u32>, Vec<FailureRecord>>,
}
struct Child {
    scope: u64,
    path: Vec<u32>,
    canceled: Option<u32>,
    finished: bool,
    detached: bool,
    result: Option<OperationResult>,
}
pub(super) enum Pending {
    Scope {
        scope: u64,
        result: Option<bool>,
    },
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
        if self.shielded {
            return None;
        }
        self.tasks
            .get(&self.current)
            .and_then(|t| t.canceled)
            .or_else(|| {
                self.active_scopes
                    .get(&self.current)?
                    .iter()
                    .rev()
                    .find_map(|id| self.scope_canceled.get(id).copied())
            })
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
                child
                    .canceled
                    .get_or_insert(if child.path == path { cause } else { 1 });
            }
        }
    }
    pub fn detached(&self, task: u64) -> bool {
        self.tasks.get(&task).is_some_and(|child| child.detached)
    }
    fn detach(&mut self, task: u64) -> Result<Vec<Value>, Error> {
        let child = self
            .tasks
            .get_mut(&task)
            .ok_or_else(|| invalid("unknown detached task"))?;
        child.detached = true;
        if child.finished {
            self.retire(task)
        } else {
            Ok(vec![])
        }
    }
    fn retire(&mut self, task: u64) -> Result<Vec<Value>, Error> {
        let child = self
            .tasks
            .remove(&task)
            .ok_or_else(|| invalid("unknown retired task"))?;
        let scope = self
            .scopes
            .get_mut(&child.scope)
            .ok_or_else(|| invalid("expired child scope"))?;
        scope.children.retain(|id| *id != task);
        match child.result {
            Some(OperationResult::Complete {
                value,
                mut released,
                ..
            }) => {
                if value != Value::Unit {
                    released.push(value);
                }
                Ok(released)
            }
            Some(OperationResult::FailureChain { failures, released }) => {
                scope.failures.insert(child.path, failures);
                Ok(released)
            }
            _ => Err(invalid("retired task has no completed result")),
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
            self.scope_canceled.entry(scope).or_insert(2);
            let siblings = self.scopes[&scope].children.clone();
            for sibling in siblings {
                if sibling != task {
                    self.cancel(sibling, 2);
                }
            }
        }
        let child = self.tasks.get_mut(&task).unwrap();
        child.finished = true;
        child.result = Some(result);
        let detached = child.detached;
        self.spawn_counts.remove(&task);
        self.active_scopes.remove(&task);
        if detached && !self.retire(task)?.is_empty() {
            return Err(invalid(
                "detached task completed before disposing its result",
            ));
        }
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
            if name == "task.await" {
                "task.await.start"
            } else {
                name
            },
            super::channels::request(self.tasks.current, a),
            vec![],
            context,
        )?;
        Ok(match name {
            "task.scope" => {
                self.simulation.id(arg(a, 0)?, "Tasks")?;
                let scope = self.tasks.fresh();
                self.tasks.scopes.insert(scope, ScopeState::default());
                self.tasks
                    .active_scopes
                    .entry(self.tasks.current)
                    .or_default()
                    .push(scope);
                self.task_callback(
                    Pending::Scope {
                        scope,
                        result: match &call
                            .types
                            .get(call.result_type)
                            .ok_or_else(|| invalid("scope result type"))?
                            .kind
                        {
                            hirsel_types::TypeKind::Parameter(_) => None,
                            hirsel_types::TypeKind::Named { definition, .. } => {
                                Some(*definition == hirsel_syntax::DefPath::core("Result"))
                            }
                            _ => Some(false),
                        },
                    },
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
                let ordinal = self
                    .tasks
                    .spawn_counts
                    .entry(self.tasks.current)
                    .or_default();
                path.push(*ordinal);
                *ordinal += 1;
                children.children.push(task);
                let canceled = self.tasks.canceled().map(|_| 1);
                self.tasks.tasks.insert(
                    task,
                    Child {
                        scope,
                        path,
                        canceled,
                        finished: false,
                        detached: false,
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
                    OperationResult::Complete {
                        value: Value::Unit,
                        arguments: vec![],
                        consumed: vec![1],
                        released: self.tasks.detach(task)?,
                    }
                } else if let Some(result) = self.tasks.tasks.get_mut(&task).unwrap().result.take()
                {
                    self.record_join(task, &result, context)?;
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
                OperationResult::FailureChain { mut failures, .. } => {
                    for failure in &mut failures {
                        if failure.spawn_path.is_empty() {
                            failure.spawn_path = self.tasks.path();
                        }
                    }
                    OperationResult::returned(error(failure_value(&failures, context)?))
                }
                _ => return Err(invalid("isolate callback did not complete")),
            },
            Pending::Scope {
                scope,
                result: is_result,
            } => {
                let active = self
                    .tasks
                    .active_scopes
                    .get_mut(&self.tasks.current)
                    .ok_or_else(|| invalid("missing active scope"))?;
                if active.pop() != Some(scope) {
                    return Err(invalid("scope exit order"));
                }
                let error_value = matches!(
                    &result,
                    OperationResult::Complete {
                        value: Value::Variant { variant: 1, .. },
                        ..
                    }
                );
                if error_value && is_result.is_none() {
                    return Err(Error::Unavailable("task.scope needs the concrete result nominal for an erased generic error exit".into()));
                }
                if matches!(&result, OperationResult::FailureChain { .. })
                    || error_value && is_result == Some(true)
                {
                    for child in self.tasks.scopes[&scope].children.clone() {
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
        let children = self.tasks.scopes[&scope].children.clone();
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
        let scope_state = self.tasks.scopes.remove(&scope).unwrap();
        let mut completed: Vec<_> = scope_state
            .failures
            .into_iter()
            .map(|(path, failures)| {
                (
                    path,
                    OperationResult::FailureChain {
                        failures,
                        released: vec![],
                    },
                )
            })
            .collect();
        for id in children {
            let child = self.tasks.tasks.remove(&id).unwrap();
            if let Some(result) = child.result {
                completed.push((child.path, result));
            }
        }
        completed.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, child) in completed {
            match child {
                OperationResult::Complete { value, .. } => released.push(value),
                OperationResult::FailureChain {
                    failures: chain, ..
                } => {
                    if let OperationResult::FailureChain {
                        failures: primary, ..
                    } = &mut result
                    {
                        if chain.first().map(|f| &f.spawn_path)
                            < primary.first().map(|f| &f.spawn_path)
                        {
                            let mut earlier = chain;
                            append_chain(&mut earlier, std::mem::take(primary));
                            *primary = earlier;
                        } else {
                            append_chain(primary, chain);
                        }
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
        self.tasks.scope_canceled.remove(&scope);
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
    fn record_join(
        &mut self,
        task: u64,
        result: &OperationResult,
        context: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        let output = match result {
            OperationResult::Complete { value, .. } => super::dispatch::encode_value(value),
            OperationResult::FailureChain { failures, .. } => {
                super::dispatch::encode_value(&failure_value(failures, context)?)
            }
            _ => return Err(invalid("unfinished joined task")),
        };
        self.simulation.record(
            "Tasks",
            "task.await.complete",
            task.to_le_bytes().to_vec(),
            output,
            context,
        )
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
                if let Some(result) = &result {
                    self.record_join(task, result, context)?;
                } else {
                    self.tasks.pending.insert(token, Pending::Await(task));
                }
                Ok(result)
            }
            Pending::Join { scope, .. }
                if scope != 0
                    && self.tasks.scopes[&scope]
                        .children
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
pub(super) fn append_chain(primary: &mut Vec<FailureRecord>, mut chain: Vec<FailureRecord>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Operations, TraceMode, observation::Anchor};
    use hirsel_span::{SourceMap, Span};
    use hirsel_syntax::DefPath;
    use hirsel_tir::{BodyId, RunConfiguration};
    use hirsel_types::{Type, TypeKind, TypeStore};

    fn call(
        library: &mut Library,
        name: &str,
        arguments: &[Value],
        cx: &mut OperationContext<'_>,
    ) -> OperationResult {
        let mut types = TypeStore::default();
        let unit = types.push(Type::with_kind(TypeKind::Tuple(vec![].into())));
        library
            .task_call(
                name,
                &OperationCall {
                    path: &DefPath::core(name),
                    arguments,
                    generics: &[],
                    types: &types,
                    result_type: unit,
                    site: Span::default(),
                },
                cx,
            )
            .unwrap()
    }

    // A corpus program cannot inspect the runtime's private child tables. Exercise
    // the real spawn/detach/completion hooks with both possible completion orders.
    #[test]
    fn detached_children_leave_bounded_scope_tables() {
        let run = RunConfiguration {
            entry: DefPath::core("main"),
            hash_seed: 0,
            inputs: vec![],
        };
        let sources = SourceMap::default();
        let anchor = Anchor {
            definition: run.entry.clone(),
            body: BodyId(0),
            cleanup: None,
            site: Span::default(),
        };
        let mut stdout = vec![];
        let mut trace = vec![];
        let mut cx = OperationContext {
            configuration: &run,
            sources: &sources,
            stdout: &mut stdout,
            trace: &mut trace,
            trace_mode: TraceMode::Effects,
            anchor: Some(&anchor),
        };
        let mut library = Library::new(&run, Default::default()).unwrap();
        let scope = library.tasks.fresh();
        library.tasks.scopes.insert(scope, Default::default());
        for ordinal in 0..4096 {
            let closure = Value::Closure {
                definition: DefPath::core("child"),
                body: BodyId(0),
                invoked: Some(Default::default()),
                captures: vec![],
                owned_captures: vec![],
                sizes: Default::default(),
            };
            let OperationResult::Complete { value: child, .. } = call(
                &mut library,
                "task.async",
                &[handle("Scope", scope), closure],
                &mut cx,
            ) else {
                panic!("spawn")
            };
            let task = id(&child, "Task").unwrap();
            assert_eq!(library.take_tasks().len(), 1);
            assert_eq!(library.tasks.tasks[&task].path, vec![ordinal]);
            if ordinal % 2 == 0 {
                call(
                    &mut library,
                    "task.detach",
                    &[handle("Scope", scope), child],
                    &mut cx,
                );
                library
                    .task_finished(task, OperationResult::returned(Value::Unit), &mut cx)
                    .unwrap();
            } else {
                library
                    .task_finished(task, OperationResult::returned(Value::Unit), &mut cx)
                    .unwrap();
                call(
                    &mut library,
                    "task.detach",
                    &[handle("Scope", scope), child],
                    &mut cx,
                );
            }
            assert!(
                library.tasks.tasks.is_empty(),
                "completed detached child retained at {ordinal}"
            );
            assert!(
                library.tasks.scopes[&scope].children.is_empty(),
                "scope child row retained at {ordinal}"
            );
            assert!(library.tasks.spawn_counts.len() <= 1);
            assert!(library.tasks.active_scopes.len() <= 1);
        }

        // Failures survive row retirement and are joined in spawn order even
        // when detached children finish in a different order.
        let mut ids = vec![];
        for _ in 0..3 {
            let closure = Value::Closure {
                definition: DefPath::core("child"),
                body: BodyId(0),
                invoked: Some(Default::default()),
                captures: vec![],
                owned_captures: vec![],
                sizes: Default::default(),
            };
            let OperationResult::Complete { value: child, .. } = call(
                &mut library,
                "task.async",
                &[handle("Scope", scope), closure],
                &mut cx,
            ) else {
                panic!("spawn")
            };
            ids.push(id(&child, "Task").unwrap());
            library.take_tasks();
            call(
                &mut library,
                "task.detach",
                &[handle("Scope", scope), child],
                &mut cx,
            );
        }
        for index in [2, 0, 1] {
            let failure = FailureRecord {
                failure: crate::Failure {
                    rule: hirsel_diag::RuleId::of("SEM-1"),
                    site: Span::default(),
                    line: 1,
                    kind: "Arithmetic".into(),
                },
                anchor: anchor.clone(),
                spawn_path: vec![],
                cause: None,
            };
            library
                .task_finished(
                    ids[index],
                    OperationResult::FailureChain {
                        failures: vec![failure],
                        released: vec![],
                    },
                    &mut cx,
                )
                .unwrap();
        }
        assert!(library.tasks.tasks.is_empty());
        assert!(library.tasks.scopes[&scope].children.is_empty());
        let joined = library
            .join_scope(
                Pending::Join {
                    scope,
                    result: OperationResult::returned(Value::Unit),
                    released: vec![],
                },
                &cx,
            )
            .unwrap();
        let OperationResult::FailureChain { failures, .. } = joined else {
            panic!("detached failure lost")
        };
        assert_eq!(
            failures
                .iter()
                .map(|f| f.spawn_path.clone())
                .collect::<Vec<_>>(),
            vec![vec![4096], vec![4097], vec![4098]]
        );
        assert_eq!(
            failures.iter().map(|f| f.cause).collect::<Vec<_>>(),
            vec![Some(1), Some(2), None]
        );
        assert!(library.tasks.scopes.is_empty());
    }
}
