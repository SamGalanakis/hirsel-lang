//! PROG-3 launch validation happens before the entry executes or any capability effect.
use super::{Library, SimulationConfig};
use crate::{Error, Execution, Limits, Outcome, Value, invalid, run_with_runtime};
use hirsel_span::SourceMap;
use hirsel_syntax::{DefPath, Visibility};
use hirsel_tir::{CapabilityEvent, RunConfiguration, TirProvider};
use hirsel_types::{Signature, SignatureKind, TypeInterner, TypeKind};
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaunchError {
    Execution {
        cause: Box<LaunchError>,
        stdout: Vec<u8>,
    },
    InvalidEntry(String),
    CannotGrant(DefPath),
    Runtime(String),
    Unavailable(String),
    Provider(hirsel_types::QueryError),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchedExecution {
    pub execution: Execution,
    pub exit_report: Option<crate::exit_report::Report>,
    pub events: Vec<CapabilityEvent>,
    pub final_state: super::simulation::ObservedSimulation,
}
impl LaunchedExecution {
    pub fn observation(&self, sources: &SourceMap) -> Option<crate::observation::Observable> {
        let mut observed = self.execution.observable(sources)?;
        observed.final_state = Some(self.final_state.clone());
        Some(observed)
    }
}
impl Library {
    pub fn entry_arguments(
        &mut self,
        types: &TypeInterner,
        signature: &Signature,
    ) -> Result<Vec<Value>, LaunchError> {
        if signature.metadata.visibility != Visibility::Public {
            return Err(LaunchError::InvalidEntry("entry must be pub fn".into()));
        }
        let SignatureKind::Function(f) = &signature.kind else {
            return Err(LaunchError::InvalidEntry("entry must be a function".into()));
        };
        if !f.generics.is_empty() || f.incomplete || f.foreign.is_some() {
            return Err(LaunchError::InvalidEntry(
                "entry must be complete and monomorphic".into(),
            ));
        }
        if hirsel_types::entry::entry_return(types, f.result).is_none() {
            return Err(LaunchError::InvalidEntry("entry must return Result[(), E], std@1/process.ExitCode or Result[std@1/process.ExitCode, E]".into()));
        }
        let mut roots = vec![];
        for p in &f.parameters {
            let TypeKind::Named {
                definition,
                arguments,
            } = &types
                .get(p.ty)
                .ok_or_else(|| LaunchError::InvalidEntry("missing parameter type".into()))?
                .kind
            else {
                return Err(LaunchError::InvalidEntry(
                    "entry parameters must be root capabilities".into(),
                ));
            };
            let definition = definition.path();
            if !arguments.is_empty()
                || !matches!(
                    definition.0.name.as_str(),
                    "Io" | "Args" | "Env" | "Unsafe" | "Test"
                )
                || *definition != hirsel_syntax::DefPath::core(&definition.0.name)
            {
                return Err(LaunchError::InvalidEntry(
                    "entry parameters must be nominal root capabilities".into(),
                ));
            }
            if definition.0.name != "Test"
                && !self.simulation.config.grants.contains(&definition.0.name)
            {
                return Err(LaunchError::CannotGrant(definition.clone()));
            }
            roots.push(definition.0.name.clone());
        }
        // Validate all grants first, so an unavailable later root cannot partly launch.
        self.entry_signature = Some(std::sync::Arc::new(signature.clone()));
        roots
            .iter()
            .map(|r| {
                self.simulation
                    .grant_root(r)
                    .map_err(|_| LaunchError::CannotGrant(hirsel_syntax::DefPath::core(r)))
            })
            .collect()
    }
}
pub fn launch(
    provider: &mut dyn TirProvider,
    types: &Arc<TypeInterner>,
    sources: &SourceMap,
    signature: &Signature,
    run: &RunConfiguration,
    config: SimulationConfig,
    limits: Limits,
) -> Result<LaunchedExecution, LaunchError> {
    launch_with_runtime(
        provider,
        sources,
        signature,
        run,
        config,
        limits,
        &core_runtime_types(types.clone()),
    )
}
/// Drivers append declared application type facts before launching.
pub fn core_runtime_types(types: Arc<TypeInterner>) -> crate::aggregate::RuntimeTypes {
    crate::aggregate::RuntimeTypes {
        nominals: vec![crate::aggregate::Nominal {
            definition: hirsel_syntax::DefPath::core("Result"),
            resource: false,
            distinct: false,
            finalizer: None,
            fields: 1,
            representation: None,
        }],
        ..crate::aggregate::RuntimeTypes::new(types)
    }
}
pub fn launch_with_runtime(
    provider: &mut dyn TirProvider,
    sources: &SourceMap,
    signature: &Signature,
    run: &RunConfiguration,
    config: SimulationConfig,
    limits: Limits,
    runtime: &crate::aggregate::RuntimeTypes,
) -> Result<LaunchedExecution, LaunchError> {
    if *signature.definition.path() != run.entry {
        return Err(LaunchError::InvalidEntry(
            "selected signature does not name entry".into(),
        ));
    }
    let mut library = Library::new(run, config).map_err(runtime_error)?;
    let arguments = library.entry_arguments(&runtime.types, signature)?;
    let execution = run_with_runtime(
        provider,
        sources,
        run,
        arguments,
        limits,
        &mut library,
        runtime,
    )
    .map_err(|error| LaunchError::Execution {
        cause: Box::new(runtime_error(error.error)),
        stdout: error.stdout,
    })?;
    if !matches!(
        execution.outcome,
        Outcome::UndefinedBehavior(_) | Outcome::Trapped(_)
    ) {
        library
            .simulation
            .finish_replay()
            .map_err(|error| execution_error(error, &execution))?;
    }
    if let Outcome::Returned { value, exit_code } = &execution.outcome {
        use hirsel_types::entry::EntryReturn;
        let SignatureKind::Function(function) = &signature.kind else {
            unreachable!()
        };
        let kind = hirsel_types::entry::entry_return(&runtime.types, function.result).unwrap();
        let code = |value: &Value| match value {
            Value::Record { fields, .. } if fields.len() == 1 => match fields[0].as_ref()? {
                Value::Integer(n) if n.kind() == hirsel_types::Primitive::U8 => {
                    Some(n.bits() as i32)
                }
                _ => None,
            },
            _ => None,
        };
        let actual = if kind == EntryReturn::ExitCode {
            code(value)
        } else {
            match value {
                Value::Variant { variant, fields } if fields.len() == 1 => {
                    match (*variant, fields[0].as_ref()) {
                        (1, Some(_)) => Some(1),
                        (0, Some(Value::Unit)) if kind == EntryReturn::UnitResult => Some(0),
                        (0, Some(value)) if kind == EntryReturn::ExitCodeResult => code(value),
                        _ => None,
                    }
                }
                _ => None,
            }
        };
        if actual != Some(*exit_code) {
            return Err(execution_error(
                invalid("entry return shape/exit mismatch"),
                &execution,
            ));
        }
    }
    Ok(LaunchedExecution {
        exit_report: library.exit_report.take(),
        execution,
        final_state: library.simulation.observed_state(),
        events: library.simulation.events,
    })
}
fn runtime_error(e: Error) -> LaunchError {
    match e {
        Error::InvalidTir(m) => LaunchError::Runtime(format!("invalid TIR: {m}")),
        Error::Unavailable(m) => LaunchError::Unavailable(m),
        Error::Query(q) => LaunchError::Provider(q),
        // Checked execution reports traps as an outcome; this is unreachable.
        Error::Blocked(_) => LaunchError::Runtime("unhandled Blocked stop signal".into()),
        Error::BudgetExceeded(_) => LaunchError::Runtime("unhandled budget stop signal".into()),
        Error::Signaled(_) => LaunchError::Runtime("unhandled signal stop".into()),
        Error::Trapped(trap) => LaunchError::Runtime(trap.to_string()),
        Error::Environmental(cause) => LaunchError::Runtime(format!("environmental: {cause}")),
        Error::ProviderFailed(record) => {
            LaunchError::Runtime(format!("unhandled provider failure: {:?}", record.failure))
        }
        Error::Ended(failure) => LaunchError::Runtime(format!("unhandled run end: {failure:?}")),
    }
}

fn execution_error(error: Error, execution: &Execution) -> LaunchError {
    LaunchError::Execution {
        cause: Box::new(runtime_error(error)),
        stdout: execution.stdout.clone(),
    }
}
