//! Implementation crates expose free functions; the engine composes those it may
//! depend on and reaches the rest through [`Phases`].
use crate::*;
use std::sync::Arc;

pub struct LexerAdapter;
impl LexerPhase for LexerAdapter {
    fn lex(
        &self,
        file: FileId,
        sources: &SourceMap,
    ) -> Result<QueryOutput<Vec<Token>>, QueryError> {
        let _timing = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Lex);
        let result = hirsel_lexer::lex(file, sources);
        Ok(QueryOutput {
            value: result.tokens,
            diagnostics: result.diagnostics,
            dependencies: vec![],
        })
    }
}
pub struct ParserAdapter;
impl ParserPhase for ParserAdapter {
    fn parse(
        &self,
        file: FileId,
        tokens: Vec<Token>,
        context: &FileContext,
        sources: &mut SourceMap,
    ) -> Result<QueryOutput<SyntaxFile>, QueryError> {
        let result = hirsel_parser::parse(file, tokens, context, sources);
        if let Some(span) = result.nesting_limit {
            let location = sources.location(span);
            let at = sources.resolve(span);
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "parser-tooling-limit".into(),
                message: format!(
                    "tree height exceeds {} in {} at {:?} (bytes {}..{}); incomplete syntax judgment (issue #33)",
                    hirsel_parser::MAX_PARSE_DEPTH,
                    sources
                        .file(at.file)
                        .map(|f| f.path.display().to_string())
                        .unwrap_or_else(|| "<unknown>".into()),
                    location,
                    at.start,
                    at.end
                ),
            }));
        }
        Ok(QueryOutput {
            value: result.syntax,
            diagnostics: result.diagnostics,
            dependencies: vec![],
        })
    }
}

/// Sources, root arguments and limits are explicit; no capabilities are granted implicitly.
pub struct InterpreterAdapter {
    pub entry_signature: Option<hirsel_types::SignatureRef>,
    pub runtime: hirsel_interp::aggregate::RuntimeTypes,
    pub simulation: hirsel_interp::library::SimulationConfig,
    pub sources: SourceMap,
    pub arguments: Vec<hirsel_interp::Value>,
    pub limits: hirsel_interp::Limits,
}
impl Default for InterpreterAdapter {
    /// An empty placeholder interner: `prepare` installs the session's runtime before a run.
    fn default() -> Self {
        Self {
            entry_signature: None,
            runtime: hirsel_interp::aggregate::RuntimeTypes::new(hirsel_types::TypeInterner::new(
                None,
            )),
            simulation: Default::default(),
            sources: Default::default(),
            arguments: Vec::new(),
            limits: Default::default(),
        }
    }
}
impl InterpreterPhase for InterpreterAdapter {
    fn replace_step_limit(
        &mut self,
        limit: Option<u64>,
    ) -> Result<Option<u64>, crate::NotImplemented> {
        Ok(std::mem::replace(&mut self.limits.steps, limit))
    }

    fn prepare(
        &mut self,
        signature: hirsel_types::SignatureRef,
        runtime: hirsel_interp::aggregate::RuntimeTypes,
        simulation: hirsel_interp::library::SimulationConfig,
    ) {
        self.entry_signature = Some(signature);
        self.runtime = runtime;
        self.simulation = simulation;
    }
    fn run(
        &mut self,
        config: &RunConfiguration,
        definitions: &mut dyn TirProvider,
        stdout: &mut dyn std::io::Write,
    ) -> Result<RunOutcome, QueryError> {
        let sources = self.sources.clone();
        self.run_query(config, definitions, stdout, &sources)
            .map_err(|error| match error {
                RuntimeError::Query(error) => error,
                // Legacy callers receive honest operational errors. Production uses
                // run_query to retain the richer driver-owned IO/tool envelope.
                RuntimeError::Tool(message) => {
                    QueryError::Operational(hirsel_types::OperationalFailure {
                        operation: "interpret".into(),
                        message,
                    })
                }
                RuntimeError::Io(error) => {
                    QueryError::Operational(hirsel_types::OperationalFailure {
                        operation: "stdout".into(),
                        message: error.to_string(),
                    })
                }
            })
    }
    fn run_query(
        &mut self,
        config: &RunConfiguration,
        definitions: &mut dyn TirProvider,
        stdout: &mut dyn std::io::Write,
        sources: &SourceMap,
    ) -> Result<RunOutcome, RuntimeError> {
        let observed = self.run_observed_query(config, definitions, sources)?;
        if let Some(observation) = &observed.observation {
            stdout
                .write_all(&observation.output)
                .map_err(RuntimeError::Io)?;
        } else {
            stdout
                .write_all(&observed.tooling_output)
                .map_err(RuntimeError::Io)?;
        }
        observed.outcome
    }
    fn run_observed_query(
        &mut self,
        config: &RunConfiguration,
        definitions: &mut dyn TirProvider,
        sources: &SourceMap,
    ) -> Result<ObservedExecution, RuntimeError> {
        let mut final_state = None;
        let mut exit_report = None;
        let execution = if let Some(signature) = &self.entry_signature {
            hirsel_interp::library::launch_with_runtime(
                definitions,
                sources,
                signature,
                config,
                self.simulation.clone(),
                self.limits,
                &self.runtime,
            )
            .map(|mut launched| {
                exit_report = launched.exit_report.take();
                final_state = Some(launched.final_state);
                launched.execution
            })
            .map_err(|error| match error {
                hirsel_interp::library::LaunchError::Execution { cause, stdout } => {
                    (launch_error(*cause), Some(stdout))
                }
                error => (launch_error(error), None),
            })
        } else {
            hirsel_interp::run(
                definitions,
                &self.runtime.types,
                sources,
                config,
                self.arguments.clone(),
                self.limits,
            )
            .map_err(|error| (core_error(error.error), Some(error.stdout)))
        };
        let execution = match execution {
            Ok(execution) => execution,
            Err((error, Some(stdout))) => {
                return Ok(ObservedExecution {
                    step_usage: None,
                    race_candidates: vec![],
                    exit_report: None,
                    tooling_stderr: vec![],
                    outcome: Err(error),
                    observation: None,
                    tooling_output: stdout,
                });
            }
            Err((error, None)) => return Err(error),
        };
        let step_usage = Some(crate::StepUsage {
            limit: self.limits.steps,
            consumed: execution.steps,
        });
        let mut observation = execution.observable(sources);
        if let Some(observation) = &mut observation {
            observation.final_state = final_state.take();
            if let Some(signature) = &self.entry_signature {
                if let hirsel_types::SignatureKind::Function(function) = &signature.kind {
                    match hirsel_types::encoding::canonical_type_encoding(
                        &self.runtime.types,
                        function.result,
                    ) {
                        Ok(encoded) => observation.returned_type = Some(encoded),
                        Err(error) => {
                            return Ok(ObservedExecution {
                                step_usage,
                                race_candidates: vec![],
                                exit_report: None,
                                tooling_stderr: vec![],
                                outcome: Err(RuntimeError::Tool(format!(
                                    "observation result type: {error:?}"
                                ))),
                                observation: None,
                                tooling_output: execution.stdout,
                            });
                        }
                    }
                }
            }
        }
        let outcome = project_outcome(&execution, sources)?;
        let race_candidates = execution
            .trace
            .iter()
            .filter_map(|event| match event {
                hirsel_interp::TraceEvent::RaceCandidate(candidate) => Some(candidate.clone()),
                _ => None,
            })
            .collect();
        Ok(ObservedExecution {
            step_usage,
            race_candidates,
            exit_report,
            tooling_stderr: final_state.map(|s| s.stderr).unwrap_or_default(),
            tooling_output: if observation.is_none() {
                execution.stdout
            } else {
                vec![]
            },
            outcome: Ok(outcome),
            observation,
        })
    }
}
pub struct FormatterAdapter;
impl FormatterPhase for FormatterAdapter {
    fn format(&self, file: &SyntaxFile, sources: &SourceMap) -> Result<String, Vec<Diagnostic>> {
        Ok(hirsel_fmt::format(file, sources).source)
    }
    fn format_query(&self, file: &SyntaxFile, sources: &SourceMap) -> Result<String, QueryError> {
        // parse_file already rejects errors, including diagnostics without an AST Error node.
        let result = hirsel_fmt::format(file, sources);
        if result.unchanged {
            Err(QueryError::NotImplemented)
        } else {
            Ok(result.source)
        }
    }
}
impl PhaseAdapters {
    /// The complete compiler: the engine's own adapters around `phases`, all
    /// using immutable phase adapters.
    pub fn compiler(phases: Arc<dyn Phases>) -> Self {
        Self {
            lexer: Some(Arc::new(LexerAdapter)),
            parser: Some(Arc::new(ParserAdapter)),
            resolver: Some(Arc::new(ResolverAdapter)),
            signature: Some(Arc::new(SignatureAdapter {
                phases: phases.clone(),
            })),
            checker: Some(phases.checker()),
            modes: Some(phases.modes()),
            lowering: Some(phases.lowering()),
            interpreter: Some(Box::new(InterpreterAdapter::default())),
            formatter: Some(Arc::new(FormatterAdapter)),
            phases: Some(phases),
        }
    }
}

fn core_error(error: hirsel_interp::Error) -> RuntimeError {
    match error {
        hirsel_interp::Error::Query(e) => RuntimeError::Query(e),
        hirsel_interp::Error::Unavailable(_) => RuntimeError::Query(QueryError::NotImplemented),
        hirsel_interp::Error::InvalidTir(e) => RuntimeError::Tool(format!("invalid TIR: {e}")),
        hirsel_interp::Error::Blocked(_) => {
            RuntimeError::Tool("unhandled Blocked stop signal".into())
        }
        hirsel_interp::Error::BudgetExceeded(_) => {
            RuntimeError::Tool("unhandled budget stop signal".into())
        }
        hirsel_interp::Error::Signaled(_) => {
            RuntimeError::Tool("unhandled signal stop signal".into())
        }
        hirsel_interp::Error::Trapped(trap) => RuntimeError::Tool(trap.to_string()),
        hirsel_interp::Error::Environmental(cause) => RuntimeError::Tool(cause.to_string()),
        hirsel_interp::Error::Ended(failure) => {
            RuntimeError::Tool(format!("unhandled run end: {failure:?}"))
        }
    }
}
fn launch_error(error: hirsel_interp::library::LaunchError) -> RuntimeError {
    match error {
        hirsel_interp::library::LaunchError::Provider(e) => RuntimeError::Query(e),
        hirsel_interp::library::LaunchError::Unavailable(_) => {
            RuntimeError::Query(QueryError::NotImplemented)
        }
        error => RuntimeError::Tool(format!("launch: {error:?}")),
    }
}

fn locate(
    sources: &hirsel_span::SourceMap,
    definition: &hirsel_syntax::DefPath,
    span: hirsel_span::Span,
) -> Option<hirsel_run_schema::Site> {
    let location = sources.location(span)?;
    Some(hirsel_run_schema::Site {
        definition: definition.to_string(),
        file: sources
            .file(sources.resolve(span).file)?
            .logical_path()
            .to_string_lossy()
            .into_owned(),
        line: location.line.into(),
        column: location.column.into(),
    })
}

/// A language failure's chain, primary first, as the host sees it: a run's ending, or a
/// callee run's (RUN-10).
pub fn project_failures(
    failures: &[hirsel_interp::observation::FailureRecord],
    sources: &hirsel_span::SourceMap,
) -> Result<RunOutcome, RuntimeError> {
    let mut failures = failures.iter().map(|failure| {
        Ok(hirsel_run_schema::Failure {
            rule: failure.failure.rule.to_string(),
            site: locate(
                sources,
                failure.anchor.definition.path(),
                failure.anchor.site,
            )
            .ok_or_else(|| RuntimeError::Tool("failure has no source site".into()))?,
            operation: failure
                .operation
                .as_ref()
                .map(|anchor| {
                    locate(sources, anchor.definition.path(), anchor.site).ok_or_else(|| {
                        RuntimeError::Tool("failure operation has no source site".into())
                    })
                })
                .transpose()?,
        })
    });
    Ok(RunOutcome::Failed {
        primary: failures
            .next()
            .ok_or_else(|| RuntimeError::Tool("failure has no primary record".into()))??,
        causes: failures.collect::<Result<_, RuntimeError>>()?,
    })
}

/// Resolve an evaluator ending into the shared host outcome. Both simulation and live
/// interpretation use this projection, retaining the complete failure chain and waits.
pub fn project_outcome(
    execution: &hirsel_interp::Execution,
    sources: &hirsel_span::SourceMap,
) -> Result<RunOutcome, RuntimeError> {
    use hirsel_run_schema::{ExecutionDiagnostic, Returned, Site, ToolingLimit, Wait};
    let locate = |definition: &hirsel_syntax::DefPath, span| locate(sources, definition, span);
    Ok(match &execution.outcome {
        hirsel_interp::Outcome::Returned { exit_code, .. } => {
            RunOutcome::Returned(Returned::from_exit_code(*exit_code as u8))
        }
        hirsel_interp::Outcome::Failed(_) => project_failures(&execution.failures, sources)?,
        hirsel_interp::Outcome::Blocked(waits) => {
            let mut waits = waits
                .iter()
                .map(|wait| {
                    Ok(Wait {
                        spawn_path: wait.spawn_path.clone(),
                        site: locate(wait.anchor.definition.path(), wait.anchor.site).ok_or_else(
                            || RuntimeError::Tool("Blocked wait has no source site".into()),
                        )?,
                        kind: wait.kind.clone(),
                        targets: wait.targets.clone(),
                    })
                })
                .collect::<Result<Vec<_>, RuntimeError>>()?;
            waits.sort_by(|a, b| a.spawn_path.cmp(&b.spawn_path));
            RunOutcome::Blocked(waits)
        }
        hirsel_interp::Outcome::BudgetExceeded(kind) => RunOutcome::BudgetExceeded(*kind),
        hirsel_interp::Outcome::Signaled(kind) => RunOutcome::Signaled(*kind),
        hirsel_interp::Outcome::Environmental(cause) => RunOutcome::Environmental(cause.clone()),
        hirsel_interp::Outcome::ToolingLimit(limit) => RunOutcome::ToolingLimit(ToolingLimit {
            kind: format!("{:?}", limit.kind),
            limit: limit.limit,
            site: limit.site.and_then(|span| {
                let location = sources.location(span)?;
                Some(Site {
                    file: sources
                        .file(sources.resolve(span).file)?
                        .logical_path()
                        .to_string_lossy()
                        .into_owned(),
                    line: location.line.into(),
                    column: location.column.into(),
                    ..Default::default()
                })
            }),
        }),
        hirsel_interp::Outcome::UndefinedBehavior(diagnostic) => {
            RunOutcome::UndefinedBehavior(ExecutionDiagnostic {
                rule: diagnostic.kind.clone(),
                message: diagnostic.to_string(),
                site: locate(&diagnostic.operation, diagnostic.site).unwrap_or_default(),
            })
        }
        hirsel_interp::Outcome::Trapped(diagnostic) => RunOutcome::Trapped(ExecutionDiagnostic {
            rule: diagnostic.rule.to_string(),
            message: diagnostic.to_string(),
            site: locate(&diagnostic.definition, diagnostic.site).unwrap_or_default(),
        }),
    })
}
