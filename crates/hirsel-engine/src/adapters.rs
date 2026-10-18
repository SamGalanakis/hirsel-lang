//! Implementation crates expose free functions; the engine composes those it may
//! depend on and reaches the rest through [`Phases`].
use crate::*;
use std::rc::Rc;

pub struct LexerAdapter;
impl LexerPhase for LexerAdapter {
    fn lex(
        &self,
        file: FileId,
        sources: &SourceMap,
        symbols: &mut Interner,
    ) -> Result<QueryOutput<Vec<Token>>, QueryError> {
        let result = hirsel_lexer::lex(file, sources, symbols);
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
        sources: &SourceMap,
        symbols: &Interner,
    ) -> Result<QueryOutput<SyntaxFile>, QueryError> {
        let result = hirsel_parser::parse(file, tokens, context, sources, symbols);
        if let Some(span) = result.nesting_limit {
            let location = sources.location(span);
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "parser-tooling-limit".into(),
                message: format!(
                    "tree height exceeds {} in {} at {:?} (bytes {}..{}); incomplete syntax judgment (issue #33)",
                    hirsel_parser::MAX_PARSE_DEPTH,
                    sources
                        .file(span.file)
                        .map(|f| f.path.display().to_string())
                        .unwrap_or_else(|| "<unknown>".into()),
                    location,
                    span.start,
                    span.end
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
#[derive(Default)]
pub struct InterpreterAdapter {
    pub entry_signature: Option<hirsel_types::SignatureRef>,
    pub runtime: hirsel_interp::aggregate::RuntimeTypes,
    pub simulation: hirsel_interp::library::SimulationConfig,
    pub sources: SourceMap,
    pub arguments: Vec<hirsel_interp::Value>,
    pub limits: hirsel_interp::Limits,
}
impl InterpreterPhase for InterpreterAdapter {
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
                    exit_report: None,
                    tooling_stderr: vec![],
                    outcome: Err(error),
                    observation: None,
                    tooling_output: stdout,
                });
            }
            Err((error, None)) => return Err(error),
        };
        let mut observation = execution.observable(sources);
        if let Some(observation) = &mut observation {
            observation.final_state = final_state.take();
            if let Some(signature) = &self.entry_signature {
                if let hirsel_types::SignatureKind::Function(function) = &signature.kind {
                    match hirsel_types::encoding::canonical_type_encoding(
                        &signature.types,
                        function.result,
                    ) {
                        Ok(encoded) => observation.returned_type = Some(encoded),
                        Err(error) => {
                            return Ok(ObservedExecution {
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
        let outcome = match execution.outcome {
            hirsel_interp::Outcome::Blocked(_) => RunOutcome::Blocked(
                observation
                    .as_ref()
                    .and_then(|o| o.blocked.clone())
                    .unwrap_or_default(),
            ),
            hirsel_interp::Outcome::ToolingLimit(limit) => RunOutcome::ToolingLimit(limit),
            hirsel_interp::Outcome::UndefinedBehavior(detected) => {
                RunOutcome::UndefinedBehavior(detected)
            }
            hirsel_interp::Outcome::Trapped(trap) => RunOutcome::Trapped(trap),
            hirsel_interp::Outcome::Returned { exit_code, .. } => RunOutcome::Exit(exit_code),
            hirsel_interp::Outcome::Failed(failure) => RunOutcome::Failure {
                rule: failure.rule,
                line: failure.line as usize,
            },
        };
        Ok(ObservedExecution {
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
    /// sharing one compiler state.
    pub fn compiler(phases: Rc<dyn Phases>) -> Self {
        let state = Rc::new(std::cell::RefCell::new(CompilerState::default()));
        Self {
            lexer: Some(Rc::new(LexerAdapter)),
            parser: Some(Rc::new(ParserAdapter)),
            resolver: Some(Rc::new(ResolverAdapter {
                state: state.clone(),
            })),
            signature: Some(Rc::new(SignatureAdapter {
                state: state.clone(),
                phases: phases.clone(),
            })),
            checker: Some(phases.checker()),
            modes: Some(phases.modes(state.clone())),
            lowering: Some(phases.lowering()),
            compiler_state: Some(state),
            interpreter: Some(Box::new(InterpreterAdapter::default())),
            formatter: Some(Rc::new(FormatterAdapter)),
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
        hirsel_interp::Error::Trapped(trap) => RuntimeError::Tool(trap.to_string()),
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
