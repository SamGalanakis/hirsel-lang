//! Closed batches follow the native CPU body's scope, receipts and task order.
use super::{
    Library,
    callbacks::Pending,
    collection,
    dispatch::{arg, int, variant},
    error, ok,
};
use crate::{
    BlockedWait, Error, OperationCall, OperationContext, OperationResult, Operations, Value,
    invalid,
};
use hirsel_free::phase::Phase;
use hirsel_types::BuiltinType;
use std::collections::VecDeque;

enum Stage {
    Scope,
    Prepare,
    Receipt,
    Preclaim,
    Spawn,
    Join,
    Joining,
    Exit,
    Exiting(u64),
    Leave,
}

pub(super) struct Batch {
    width: u32,
    scope: u64,
    stage: Stage,
    prepare: Option<Value>,
    mapper: Value,
    jobs: Option<Value>,
    next: usize,
    children: VecDeque<u64>,
    results: Vec<Value>,
    outcome: Option<OperationResult>,
    released: Vec<Value>,
}
impl Batch {
    pub(super) fn bytes(&self) -> u64 {
        self.jobs
            .as_ref()
            .map_or(0, Value::logical_bytes)
            .saturating_add(self.results.iter().map(Value::logical_bytes).sum::<u64>())
            .saturating_add(
                self.outcome
                    .as_ref()
                    .map_or(0, crate::resources::result_bytes),
            )
            .saturating_add(self.released.iter().map(Value::logical_bytes).sum::<u64>())
    }
    pub(super) fn release(mut self) -> Vec<Value> {
        self.released.extend(self.jobs.take());
        self.released.extend(self.results);
        self.released
    }
    fn refuse(&mut self, status: u32) {
        self.outcome = Some(refused(status));
        self.stage = Stage::Exit;
    }
}
fn refused(status: u32) -> OperationResult {
    let payload = match status {
        1..=8 => variant(status - 1, vec![]),
        10 => variant(10, vec![]),
        9 | 11 => variant(status, vec![]),
        16..=19 => variant(8, vec![variant(status - 16, vec![])]),
        _ => unreachable!("CPU control status {status}"),
    };
    OperationResult::returned(error(payload))
}

impl Library {
    /// Match cpu_scoped.rs's request and refusal precedence. Replay supplies the
    /// recorded status without consulting physical capacity or starting workers.
    fn cpu_control(
        &mut self,
        width: u32,
        action: u32,
        count: u64,
        context: &OperationContext<'_>,
    ) -> Result<Result<u32, OperationResult>, Error> {
        if let super::free::Enter::Park(park) = self.free_enter(Phase::Done, context)? {
            return Ok(Err(park));
        }
        let live = match action {
            0 | 4 => {
                if !matches!(width, 1 | 4 | 8 | 16) {
                    1
                } else if self.simulation.config.cpu.is_none() {
                    2
                } else if self.cpu_active.contains(&self.tasks.current) {
                    5
                } else if !self.simulation.config.tasks_free && action != 4 {
                    11
                } else if let Some(cause) = self.tasks.canceled() {
                    16 + cause
                } else if self
                    .simulation
                    .config
                    .cpu
                    .as_ref()
                    .is_some_and(|g| width > g.max_workers)
                {
                    4
                } else if !self.cpu_active.is_empty() {
                    3
                } else {
                    0
                }
            }
            1 => self.tasks.canceled().map_or(0, |cause| 16 + cause),
            2 | 5 => {
                if (action == 2 && count > u64::from(width) * 2)
                    || (action == 5 && count >= u64::from(width) * 2)
                {
                    6
                } else {
                    self.tasks.canceled().map_or(0, |cause| 16 + cause)
                }
            }
            _ => return Err(invalid("unknown interpreter CPU control")),
        };
        let path = self.tasks.path();
        let status = match &mut self.free {
            Some(free) => free
                .session
                .cpu_control(&path, width, action, count, live)
                .map_err(|e| invalid(format!("free-mode {e}")))?,
            None => live,
        };
        if matches!(action, 0 | 4) && status == 0 {
            self.cpu_active.insert(self.tasks.current);
        }
        if action == 1 {
            self.cpu_active.remove(&self.tasks.current);
        }
        let done = self.free_done(OperationResult::returned(Value::Unit), context)?;
        if matches!(done, OperationResult::Park { .. }) {
            return Ok(Err(done));
        }
        self.tasks.yield_now = true;
        Ok(Ok(status))
    }

    pub(super) fn cpu_call(
        &mut self,
        call: &OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        self.simulation.id(arg(call.arguments, 0)?, "Tasks")?;
        let width = int(arg(call.arguments, 1)?)?.bits() as u32;
        let status =
            match self.cpu_control(width, if self.cpu_certified { 4 } else { 0 }, 0, context)? {
                Ok(status) => status,
                Err(park) => return Ok(park),
            };
        if status != 0 {
            return Ok(refused(status));
        }
        Ok(self.cpu_pause(
            Batch {
                width,
                scope: 0,
                stage: Stage::Scope,
                prepare: Some(arg(call.arguments, 2)?.clone()),
                mapper: arg(call.arguments, 3)?.clone(),
                jobs: None,
                next: 0,
                children: VecDeque::new(),
                results: vec![],
                outcome: None,
                released: vec![],
            },
            context,
        ))
    }

    fn cpu_pause(&mut self, batch: Batch, context: &OperationContext<'_>) -> OperationResult {
        let token = self.next_callback;
        self.next_callback += 1;
        self.cpu_waits.insert(token, batch);
        self.tasks.yield_now = true;
        OperationResult::Park {
            token,
            wait: BlockedWait {
                spawn_path: self.tasks.path(),
                anchor: context.anchor().expect("CPU invocation anchor").clone(),
                kind: "cpu.map_scoped".into(),
                targets: vec![],
            },
        }
    }

    pub(super) fn cpu_resume(
        &mut self,
        mut batch: Batch,
        result: OperationResult,
        _arguments: &[Value],
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        match result {
            OperationResult::Complete {
                value, released, ..
            } => {
                batch.jobs = Some(value);
                batch.released.extend(released);
                batch.stage = Stage::Receipt;
            }
            result @ OperationResult::FailureChain { .. } => {
                batch.outcome = Some(result);
                batch.stage = Stage::Exit;
            }
            OperationResult::Failed(failure) => {
                batch.outcome = Some(OperationResult::FailureChain {
                    failures: vec![crate::observation::FailureRecord {
                        failure,
                        anchor: context
                            .anchor()
                            .ok_or_else(|| invalid("CPU failure anchor"))?
                            .clone(),
                        operation: None,
                        cause: None,
                        spawn_path: self.tasks.path(),
                    }],
                    released: vec![],
                });
                batch.stage = Stage::Exit;
            }
            _ => return Err(invalid("CPU preparation did not complete")),
        }
        Ok(self.cpu_pause(batch, context))
    }

    pub(super) fn poll_cpu(
        &mut self,
        token: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let mut batch = self
            .cpu_waits
            .remove(&token)
            .ok_or_else(|| invalid("unknown CPU wait"))?;
        match batch.stage {
            Stage::Scope => {
                if let super::free::Enter::Park(park) = self.free_enter(Phase::Done, context)? {
                    return Ok(Some(park));
                }
                batch.scope = self.cpu_scope();
                batch.stage = Stage::Prepare;
                let result = self.cpu_pause(batch, context);
                return self.free_done(result, context).map(Some);
            }
            Stage::Prepare => {
                let prepare = batch.prepare.take().expect("CPU preparation");
                let mut result = self.schedule(
                    Pending::Cpu(Box::new(batch)),
                    prepare,
                    vec![],
                    vec![],
                    vec![],
                );
                if let OperationResult::Call { consumed, .. } = &mut result {
                    consumed.push(2);
                }
                return Ok(Some(result));
            }
            Stage::Receipt => {
                let count =
                    collection::len(batch.jobs.as_ref().ok_or_else(|| invalid("CPU jobs"))?)?;
                let status = match self.cpu_control(batch.width, 2, count, context)? {
                    Ok(status) => status,
                    Err(park) => return Ok(Some(park)),
                };
                if status != 0 {
                    batch.refuse(status);
                } else {
                    batch.stage = Stage::Preclaim;
                }
            }
            Stage::Preclaim => {
                if batch.next as u64 == collection::len(batch.jobs.as_ref().unwrap())? {
                    batch.stage = Stage::Join;
                } else {
                    let status =
                        match self.cpu_control(batch.width, 5, batch.next as u64, context)? {
                            Ok(status) => status,
                            Err(park) => return Ok(Some(park)),
                        };
                    if status != 0 {
                        batch.refuse(status);
                    } else {
                        batch.stage = Stage::Spawn;
                    }
                }
            }
            Stage::Spawn => {
                if let super::free::Enter::Park(park) = self.free_enter(Phase::Done, context)? {
                    return Ok(Some(park));
                }
                let Some(Value::Sequence {
                    kind: BuiltinType::List,
                    elements,
                    ..
                }) = &mut batch.jobs
                else {
                    return Err(invalid("CPU preparation must return List"));
                };
                let job = elements
                    .get_mut(batch.next)
                    .and_then(Option::take)
                    .ok_or_else(|| invalid("CPU job moved twice"))?;
                let child =
                    self.spawn_child(batch.scope, batch.mapper.clone(), vec![job], true, context)?;
                batch.children.push_back(child);
                batch.next += 1;
                batch.stage = Stage::Preclaim;
                let result = self.cpu_pause(batch, context);
                return self.free_done(result, context).map(Some);
            }
            Stage::Join => {
                if batch.children.is_empty() {
                    batch.stage = Stage::Exit;
                } else {
                    if let super::free::Enter::Park(park) = self.free_enter_call(context)? {
                        return Ok(Some(park));
                    }
                    batch.stage = Stage::Joining;
                }
            }
            Stage::Joining => {
                let child = *batch.children.front().expect("CPU child");
                let Some(result) = self.cpu_child_result(child) else {
                    self.cpu_waits.insert(token, batch);
                    return Ok(None);
                };
                self.record_join(child, &result, context)?;
                batch.children.pop_front();
                match result {
                    OperationResult::Complete {
                        value, released, ..
                    } => {
                        batch.results.push(value);
                        batch.released.extend(released);
                        batch.stage = Stage::Join;
                    }
                    result @ OperationResult::FailureChain { .. } => {
                        batch.outcome = Some(result);
                        batch.stage = Stage::Exit;
                    }
                    _ => return Err(invalid("CPU child did not finish")),
                }
                let result = self.cpu_pause(batch, context);
                return self.free_done(result, context).map(Some);
            }
            Stage::Exit => {
                let result = batch.outcome.take().unwrap_or_else(|| {
                    OperationResult::returned(ok(collection::sequence(
                        BuiltinType::List,
                        std::mem::take(&mut batch.results),
                    )))
                });
                let result = self.resume_task(
                    super::tasks::Pending::Scope {
                        scope: batch.scope,
                        result: Some(true),
                    },
                    result,
                    context,
                )?;
                match result {
                    OperationResult::Park { token: wait, .. } => batch.stage = Stage::Exiting(wait),
                    result => {
                        batch.outcome = Some(result);
                        batch.stage = Stage::Leave;
                    }
                }
            }
            Stage::Exiting(wait) => {
                let Some(result) = self.poll(wait, context)? else {
                    self.cpu_waits.insert(token, batch);
                    return Ok(None);
                };
                batch.outcome = Some(result);
                batch.stage = Stage::Leave;
            }
            Stage::Leave => {
                let status = match self.cpu_control(batch.width, 1, 0, context)? {
                    Ok(status) => status,
                    Err(park) => return Ok(Some(park)),
                };
                let mut result = batch.outcome.take().ok_or_else(|| invalid("CPU outcome"))?;
                if status != 0
                    && matches!(
                        result,
                        OperationResult::Complete {
                            value: Value::Variant { variant: 0, .. },
                            ..
                        }
                    )
                {
                    if let OperationResult::Complete { value, .. } = result {
                        batch.released.push(value);
                    }
                    result = refused(status);
                }
                let released = batch.release();
                match &mut result {
                    OperationResult::Complete {
                        released: pending, ..
                    }
                    | OperationResult::FailureChain {
                        released: pending, ..
                    } => pending.extend(released),
                    _ => return Err(invalid("invalid CPU scope result")),
                }
                return Ok(Some(result));
            }
        }
        Ok(Some(self.cpu_pause(batch, context)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TraceMode, observation::Anchor};
    use hirsel_free::{
        projection::Observed,
        session::{Recording, Session},
    };
    use hirsel_span::{SourceMap, Span};
    use hirsel_syntax::DefPath;
    use hirsel_tir::{BodyId, RunConfiguration};

    #[test]
    fn owned_jobs_have_child_paths_and_free_receipts_replay() {
        for width in [1, 4, 8] {
            let run = RunConfiguration {
                entry: DefPath::core("main"),
                hash_seed: 0,
                inputs: vec![],
            };
            let sources = SourceMap::default();
            let anchor = Anchor {
                definition: (&run.entry).into(),
                body: BodyId(0),
                cleanup: None,
                site: Span::default(),
            };
            let mut stdout = vec![];
            let mut trace = vec![];
            let mut context = OperationContext {
                configuration: &run,
                sources: &sources,
                stdout: &mut stdout,
                trace: &mut trace,
                trace_mode: TraceMode::Effects,
                anchor: Some(&anchor),
            };
            let mut recorded: Option<(Vec<u8>, hirsel_free::projection::Projection)> = None;
            for replay in [false, true] {
                let mut library = Library::new(
                    &run,
                    super::super::SimulationConfig {
                        cpu: Some(hirsel_run_schema::CpuGrant {
                            provider: "cpu.scoped@1".into(),
                            max_workers: width,
                            max_domains: 1,
                            max_physical_bytes: 1 << 30,
                            max_batch_bytes: 8 << 20,
                        }),
                        tasks_free: true,
                        ..Default::default()
                    },
                )
                .unwrap();
                library.enable_free(
                    if replay {
                        Session::replay(&recorded.as_ref().unwrap().0).unwrap()
                    } else {
                        Session::record(b"CPU task continuation", Recording::default())
                    },
                    1,
                );
                assert!(matches!(
                    library.cpu_control(width, 0, 0, &context).unwrap(),
                    Ok(0)
                ));
                let mut result = library.cpu_pause(
                    Batch {
                        width,
                        scope: 0,
                        stage: Stage::Scope,
                        prepare: Some(Value::Unit),
                        mapper: Value::Unit,
                        jobs: None,
                        next: 0,
                        children: VecDeque::new(),
                        results: vec![],
                        outcome: None,
                        released: vec![],
                    },
                    &context,
                );
                let mut spawned = 0;
                let mut finished = false;
                for _ in 0..256 {
                    for start in library.take_tasks() {
                        assert_eq!(start.parent, 0);
                        assert!(start.borrowed);
                        assert_eq!(library.ownership_task_path(start.id), Some(vec![spawned]));
                        assert_eq!(
                            start.arguments,
                            vec![super::super::u64_value(u64::from(spawned + 1))]
                        );
                        spawned += 1;
                        library.enter_task(start.id, false);
                        library
                            .task_finished(
                                start.id,
                                OperationResult::returned(super::super::u64_value(
                                    u64::from(spawned) * 10,
                                )),
                                &mut context,
                            )
                            .unwrap();
                    }
                    library.enter_task(0, false);
                    for rest in library.take_rests(0) {
                        assert!(rest.values.is_empty(), "joined jobs move their result");
                        library.enter_task(rest.child, false);
                        library.rest_finished(rest.child, vec![]).unwrap();
                    }
                    library.enter_task(0, false);
                    library
                        .rests_completed(0, &mut result, &mut context)
                        .unwrap();
                    result = match result {
                        OperationResult::Call { continuation, .. } => {
                            let jobs = collection::sequence(
                                BuiltinType::List,
                                (1..=2 * width)
                                    .map(|n| super::super::u64_value(u64::from(n)))
                                    .collect::<Vec<_>>(),
                            );
                            library
                                .resume(
                                    continuation,
                                    OperationResult::returned(jobs),
                                    &mut [],
                                    &mut context,
                                )
                                .unwrap()
                        }
                        OperationResult::Park { token, .. } => {
                            let Some(next) = library.poll(token, &mut context).unwrap() else {
                                break;
                            };
                            next
                        }
                        OperationResult::Complete { value, .. } => {
                            assert_eq!(
                                value,
                                ok(collection::sequence(
                                    BuiltinType::List,
                                    (1..=2 * width)
                                        .map(|n| super::super::u64_value(u64::from(n) * 10))
                                        .collect::<Vec<_>>()
                                ))
                            );
                            finished = true;
                            break;
                        }
                        _ => panic!("unexpected CPU result"),
                    };
                }
                assert_eq!(spawned, 2 * width);
                assert!(library.cpu_active.is_empty());
                if replay {
                    let projection = library.take_free().unwrap().finish().unwrap();
                    assert_eq!(projection, recorded.as_ref().unwrap().1);
                } else {
                    assert!(finished);
                    library.free_stop("returned");
                    let report = library.take_free().unwrap().report();
                    let projection = report.cuts.last().unwrap().1.clone();
                    assert_eq!(projection.cut.len(), 1 + 2 * width as usize);
                    let receipts = projection
                        .observed
                        .values()
                        .flatten()
                        .filter_map(|o| match o {
                            Observed::CpuControl {
                                action,
                                count,
                                outcome,
                                ..
                            } => {
                                assert_eq!(*outcome, 0);
                                Some((*action, *count))
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    let mut expected = vec![(0, 0), (2, u64::from(2 * width))];
                    expected.extend((0..2 * width).map(|n| (5, u64::from(n))));
                    expected.push((1, 0));
                    assert_eq!(receipts, expected);
                    recorded = Some((report.tape, projection));
                }
            }
        }
    }
}
