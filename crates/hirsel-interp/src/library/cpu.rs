//! Serial reference evaluation of a closed CPU batch. It certifies no CPUs.
use super::{
    Library,
    callbacks::Pending,
    collection,
    dispatch::{arg, int, variant},
    error, ok,
};
use crate::{Error, OperationCall, OperationContext, OperationResult, Value, invalid};
use hirsel_types::BuiltinType;

pub(super) enum Batch {
    Prepared {
        width: usize,
    },
    Mapped {
        jobs: Value,
        next: usize,
        results: Vec<Value>,
    },
}
impl Batch {
    pub(super) fn bytes(&self) -> u64 {
        match self {
            Self::Prepared { .. } => 0,
            Self::Mapped { jobs, results, .. } => jobs
                .logical_bytes()
                .saturating_add(results.iter().map(Value::logical_bytes).sum::<u64>()),
        }
    }
    pub(super) fn release(self) -> Vec<Value> {
        match self {
            Self::Prepared { .. } => vec![],
            Self::Mapped {
                jobs, mut results, ..
            } => {
                results.insert(0, jobs);
                results
            }
        }
    }
}
fn refused(ordinal: u32) -> OperationResult {
    OperationResult::returned(error(variant(ordinal, vec![])))
}
impl Library {
    pub(super) fn cpu_call(
        &mut self,
        call: &OperationCall<'_>,
        _context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let width = int(arg(call.arguments, 1)?)?.bits() as usize;
        if !matches!(width, 1 | 4 | 8 | 16) {
            return Ok(refused(0));
        }
        self.simulation.id(arg(call.arguments, 0)?, "Tasks")?;
        let Some(grant) = self.simulation.config.cpu.as_ref() else {
            return Ok(refused(1));
        };
        if self.cpu_active.contains(&self.tasks.current) {
            return Ok(refused(4));
        }
        if !self.cpu_active.is_empty() {
            return Ok(refused(2));
        }
        if !self.simulation.config.tasks_free {
            return Ok(refused(9));
        }
        if let Some(cause) = self.tasks.canceled() {
            return Ok(OperationResult::returned(error(variant(
                8,
                vec![variant(cause, vec![])],
            ))));
        }
        if width > grant.max_workers as usize {
            return Ok(refused(3));
        }
        self.cpu_active.insert(self.tasks.current);
        let mut result = self.schedule(
            Pending::Cpu(Batch::Prepared { width }),
            arg(call.arguments, 2)?.clone(),
            vec![],
            vec![],
            vec![],
        );
        if let OperationResult::Call { consumed, .. } = &mut result {
            consumed.push(2);
        }
        Ok(result)
    }
    pub(super) fn cpu_resume(
        &mut self,
        batch: Batch,
        result: OperationResult,
        arguments: &[Value],
        context: &OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let value = match result {
            OperationResult::Complete { value, .. } => value,
            OperationResult::FailureChain {
                failures,
                mut released,
            } => {
                self.cpu_active.remove(&self.tasks.current);
                released.extend(batch.release());
                return Ok(OperationResult::FailureChain { failures, released });
            }
            OperationResult::Failed(failure) => {
                self.cpu_active.remove(&self.tasks.current);
                return Ok(OperationResult::FailureChain {
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
                    released: batch.release(),
                });
            }
            other => {
                self.cpu_active.remove(&self.tasks.current);
                return Ok(other);
            }
        };
        let (mut jobs, next, mut results) = match batch {
            Batch::Prepared { width } => {
                let count = collection::len(&value)?;
                if count > width as u64 * 2 {
                    self.cpu_active.remove(&self.tasks.current);
                    return Ok(OperationResult::Complete {
                        value: error(variant(5, vec![])),
                        arguments: vec![],
                        consumed: vec![],
                        released: vec![value],
                    });
                }
                (value, 0, vec![])
            }
            Batch::Mapped {
                jobs,
                next,
                mut results,
            } => {
                results.push(value);
                (jobs, next, results)
            }
        };
        if let Some(cause) = self.tasks.canceled() {
            self.cpu_active.remove(&self.tasks.current);
            results.insert(0, jobs);
            return Ok(OperationResult::Complete {
                value: error(variant(8, vec![variant(cause, vec![])])),
                arguments: vec![],
                consumed: vec![],
                released: results,
            });
        }
        let Value::Sequence {
            kind: BuiltinType::List,
            elements,
            ..
        } = &mut jobs
        else {
            return Err(invalid("CPU preparation must return List"));
        };
        let Some(job) = elements.get_mut(next).and_then(Option::take) else {
            self.cpu_active.remove(&self.tasks.current);
            return Ok(OperationResult::Complete {
                value: ok(collection::sequence(BuiltinType::List, results)),
                arguments: vec![],
                consumed: vec![],
                released: vec![jobs],
            });
        };
        Ok(self.schedule(
            Pending::Cpu(Batch::Mapped {
                jobs,
                next: next + 1,
                results,
            }),
            arg(arguments, 3)?.clone(),
            vec![job],
            vec![],
            vec![],
        ))
    }
}
