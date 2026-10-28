//! CPU task boundaries observed from evaluator-owned continuations, never a native tape.
use super::{CpuControl, Library};
use crate::{Error, OperationResult, Value, invalid};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CpuPhase {
    Control(CpuControl, u32),
    ScopeEnter,
    Spawn(Vec<u32>),
    Join(Vec<u32>),
    Joined(Vec<u32>, Vec<u8>),
    End(Vec<u8>),
    Rest,
    ScopeExit,
    ScopeExited,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CpuBoundary {
    pub task: Vec<u32>,
    pub phase: CpuPhase,
}

#[derive(Default)]
pub(super) struct Trace {
    pub events: Vec<CpuBoundary>,
    pub fuel: u64,
    pub event_fuel: Vec<u64>,
    pub jobs: std::collections::BTreeSet<Vec<u32>>,
}

pub(super) fn scalar(result: &OperationResult) -> Result<Vec<u8>, Error> {
    let OperationResult::Complete { value, .. } = result else {
        return Err(invalid("CPU trace requires completed scalar evidence"));
    };
    fn encode(value: &Value, bytes: &mut Vec<u8>) -> Result<(), Error> {
        match value {
            Value::Unit => (),
            Value::Bool(value) => bytes.push(u8::from(*value)),
            Value::Integer(value) => {
                bytes.extend_from_slice(&value.bits().to_le_bytes()[..value.width() as usize / 8])
            }
            Value::F32(value) => bytes.extend_from_slice(&value.to_le_bytes()),
            Value::F64(value) => bytes.extend_from_slice(&value.to_le_bytes()),
            Value::Sequence {
                kind: hirsel_types::BuiltinType::List | hirsel_types::BuiltinType::Array,
                elements,
                arena: None,
                ..
            } => {
                bytes.extend_from_slice(&(elements.len() as u64).to_le_bytes());
                for element in elements {
                    encode(
                        element
                            .as_ref()
                            .ok_or_else(|| invalid("moved CPU sequence element"))?,
                        bytes,
                    )?;
                }
            }
            Value::Tuple(fields) | Value::Box(fields) => {
                for field in fields {
                    encode(
                        field
                            .as_ref()
                            .ok_or_else(|| invalid("moved CPU scalar field"))?,
                        bytes,
                    )?;
                }
            }
            _ => return Err(invalid("CPU trace has no scalar result encoding")),
        }
        Ok(())
    }
    // TaskFinish owns Some(value), including its initialized discriminant.
    let mut bytes = 1u32.to_le_bytes().to_vec();
    encode(value, &mut bytes)?;
    Ok(bytes)
}

impl Library {
    pub fn enable_cpu_trace(&mut self) {
        self.cpu_trace = Some(Default::default());
    }

    pub fn cpu_trace(&self) -> Option<&[CpuBoundary]> {
        self.cpu_trace.as_ref().map(|trace| trace.events.as_slice())
    }

    pub fn cpu_fuel_trace(&self) -> Option<&[u64]> {
        self.cpu_trace
            .as_ref()
            .map(|trace| trace.event_fuel.as_slice())
    }

    pub(super) fn cpu_rest(&mut self) {
        let task = self.tasks.path();
        if self
            .cpu_trace
            .as_mut()
            .is_some_and(|trace| trace.jobs.remove(&task))
        {
            self.cpu_boundary(CpuPhase::Rest);
        }
    }

    pub(super) fn cpu_boundary(&mut self, phase: CpuPhase) {
        if self.simulation.config.tasks_free {
            return;
        }
        let task = self.tasks.path();
        if let Some(trace) = &mut self.cpu_trace {
            if let CpuPhase::Spawn(path) = &phase {
                trace.jobs.insert(path.clone());
            }
            trace.event_fuel.push(trace.fuel);
            trace.events.push(CpuBoundary { task, phase });
        }
    }
}
