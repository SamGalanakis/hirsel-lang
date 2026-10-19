use crate::*;
use hirsel_syntax::PackageId;
use std::{io, path::Path};

/// Interpreter API errors remain distinct from missing implementation and language failures.
#[derive(Debug)]
pub enum RuntimeError {
    Query(QueryError),
    Tool(String),
    Io(io::Error),
}
/// Tool errors remain separate from language diagnostics (PROG-3).
#[derive(Debug)]
pub enum ToolError {
    Io(io::Error),
    SourceTooLarge,
    Runtime(String),
    /// Admission refused the run before entry (PROG-3).
    Launch(hirsel_run_config::RunRefusal),
    Query {
        error: QueryError,
        phase: Option<QueryKind>,
        sources: SourceMap,
    },
}
impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Query {
                error: QueryError::Operational(error),
                ..
            } => write!(f, "{}: {}", error.operation, error.message),
            Self::Io(error) => error.fmt(f),
            Self::Runtime(message) => f.write_str(message),
            Self::Launch(refusal) => write!(f, "launch: {refusal}"),
            Self::SourceTooLarge => f.write_str("source exceeds the u32 source-map limit"),
            Self::Query {
                error: QueryError::NotImplemented,
                phase,
                ..
            } => match phase {
                Some(phase) => write!(f, "{} is not implemented yet", phase.phase_name()),
                None => f.write_str("not implemented yet"),
            },
            Self::Query {
                error: QueryError::Diagnostics(_),
                ..
            } => f.write_str("language diagnostics"),
        }
    }
}
impl std::error::Error for ToolError {}
impl From<io::Error> for ToolError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn default_context() -> FileContext {
    // A source without an explicit launch manifest is standalone@0.
    FileContext {
        package: PackageId {
            name: "standalone".into(),
            version: "0".into(),
        },
        modules: vec![],
    }
}
/// The spec's parts under docs/spec, by file name, which the spec hash covers.
/// The conformance spec_links test checks that they are the whole directory.
pub const SPEC_PARTS: [(&str, &[u8]); 22] = [
    ("brands.md", include_bytes!("../../../docs/spec/brands.md")),
    (
        "capabilities.md",
        include_bytes!("../../../docs/spec/capabilities.md"),
    ),
    (
        "closures.md",
        include_bytes!("../../../docs/spec/closures.md"),
    ),
    (
        "compile-time.md",
        include_bytes!("../../../docs/spec/compile-time.md"),
    ),
    (
        "concurrency.md",
        include_bytes!("../../../docs/spec/concurrency.md"),
    ),
    (
        "contracts.md",
        include_bytes!("../../../docs/spec/contracts.md"),
    ),
    (
        "core-library.md",
        include_bytes!("../../../docs/spec/core-library.md"),
    ),
    (
        "determinism.md",
        include_bytes!("../../../docs/spec/determinism.md"),
    ),
    (
        "diagnostics.md",
        include_bytes!("../../../docs/spec/diagnostics.md"),
    ),
    (
        "failure.md",
        include_bytes!("../../../docs/spec/failure.md"),
    ),
    (
        "generics.md",
        include_bytes!("../../../docs/spec/generics.md"),
    ),
    (
        "implementation.md",
        include_bytes!("../../../docs/spec/implementation.md"),
    ),
    ("index.md", include_bytes!("../../../docs/spec/index.md")),
    (
        "language.md",
        include_bytes!("../../../docs/spec/language.md"),
    ),
    ("memory.md", include_bytes!("../../../docs/spec/memory.md")),
    (
        "ownership.md",
        include_bytes!("../../../docs/spec/ownership.md"),
    ),
    (
        "programs.md",
        include_bytes!("../../../docs/spec/programs.md"),
    ),
    (
        "proof-seam.md",
        include_bytes!("../../../docs/spec/proof-seam.md"),
    ),
    (
        "semantics.md",
        include_bytes!("../../../docs/spec/semantics.md"),
    ),
    (
        "serialization.md",
        include_bytes!("../../../docs/spec/serialization.md"),
    ),
    (
        "std-library.md",
        include_bytes!("../../../docs/spec/std-library.md"),
    ),
    ("syntax.md", include_bytes!("../../../docs/spec/syntax.md")),
];
pub fn default_environment() -> QueryEnvironment {
    QueryEnvironment {
        interface_version: 11,
        compiler_version: env!("CARGO_PKG_VERSION").into(),
        spec_hash: ContentHash::of_parts(
            "hirsel.spec",
            SPEC_PARTS
                .iter()
                .flat_map(|(name, text)| [name.as_bytes(), *text]),
        ),
        package_manifest_hash: ContentHash::of_bytes(b"standalone@0"),
        builtin_signatures_hash: registry::BuiltinRegistry::new().revision(),
    }
}

pub(crate) fn read_source(path: &Path) -> Result<String, ToolError> {
    let bytes = std::fs::read(path)?;
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            let offset = error.utf8_error().valid_up_to();
            // The valid prefix keeps the original byte, line and scalar column.
            // Lossy replacement would shift every following source position.
            let prefix = std::str::from_utf8(&error.as_bytes()[..offset]).unwrap();
            let mut sources = SourceMap::default();
            let file = sources
                .add(path, prefix.into())
                .ok_or(ToolError::SourceTooLarge)?;
            let span = hirsel_span::Span::new(file, offset as u32, offset as u32).unwrap();
            let mut diagnostic = Diagnostic::error(
                const { hirsel_diag::RuleId::of("SEM-3") },
                span,
                "invalid UTF-8 in source",
            );
            diagnostic
                .notes
                .push(format!("invalid byte sequence begins at byte {offset}"));
            return Err(ToolError::Query {
                error: QueryError::Diagnostics(vec![diagnostic]),
                phase: Some(QueryKind::Lex),
                sources,
            });
        }
    };
    Ok(text)
}
/// Complete declaration preflight; package launch calls it for every selected file
/// before any body can plan calls through a foreign loop or format association.
pub(crate) fn association_diagnostics(
    engine: &mut QueryEngine,
    file: FileId,
    context: &FileContext,
) -> Result<Vec<Diagnostic>, QueryError> {
    let parsed = engine.parse_file(file, context)?;
    let mut diagnostics = parsed.diagnostics;
    if crate::queries::has_errors(&diagnostics) {
        return Ok(diagnostics);
    }
    for item in &parsed.value.roots {
        let source = &parsed.value.tree.items[item.0 as usize];
        for attribute in &source.attributes {
            let name = attribute.name.as_str();
            if !matches!(name, "loop_source" | "format") {
                continue;
            }
            let phases = engine
                .adapters
                .phases
                .clone()
                .ok_or(QueryError::NotImplemented)?;
            let span = attribute.origin.span;
            let result = engine
                .item_signature(&source.definition)
                .and_then(|signature| {
                    if name == "format" {
                        phases.validate_format(&signature.value, span, engine)
                    } else {
                        phases.validate_loop_source(&signature.value, span, engine)
                    }
                });
            match result {
                Ok(()) => (),
                Err(QueryError::Diagnostics(errors)) => diagnostics.extend(errors),
                Err(error) => return Err(error),
            }
        }
    }
    Ok(diagnostics)
}

/// A partial pipeline can produce rejection diagnostics, but never a clean acceptance.
/// A standalone file is its own program package: every executable body lowers.
pub fn check_progress(
    engine: &mut QueryEngine,
    file: FileId,
    context: &FileContext,
) -> Result<CheckOutput, QueryError> {
    check_file_lowering(engine, file, context, crate::queries::Lowering::All)
}
pub(crate) fn check_file_lowering(
    engine: &mut QueryEngine,
    file: FileId,
    context: &FileContext,
    lowering: crate::queries::Lowering,
) -> Result<CheckOutput, QueryError> {
    let mut diagnostics = vec![];
    let result = (|| -> Result<(), QueryError> {
        let parsed = engine.parse_file(file, context)?;
        diagnostics.extend(association_diagnostics(engine, file, context)?);
        if crate::queries::has_errors(&diagnostics) {
            return Ok(());
        }
        for item in &parsed.value.roots {
            let source = &parsed.value.tree.items[item.0 as usize];
            match engine.mode_definition(&source.definition) {
                Ok(mode) => diagnostics.extend(mode.diagnostics),
                Err(QueryError::Diagnostics(errors)) => diagnostics.extend(errors),
                Err(error) => return Err(error),
            }
            if matches!(
                &source.kind,
                hirsel_syntax::ItemKind::Type {
                    body: hirsel_syntax::TypeBody::Distinct { fact: Some(_), .. },
                    ..
                }
            ) {
                let predicate = hirsel_check_protocol::fact::fact_path(&source.definition);
                match engine.mode_definition(&predicate) {
                    Ok(mode) => diagnostics.extend(mode.diagnostics),
                    Err(QueryError::Diagnostics(errors)) => diagnostics.extend(errors),
                    Err(error) => return Err(error),
                }
            }
        }
        if !crate::queries::has_errors(&diagnostics) {
            diagnostics.extend(crate::generic_graph::check(engine)?);
        }
        if !crate::queries::has_errors(&diagnostics) {
            for item in &parsed.value.roots {
                let source = &parsed.value.tree.items[item.0 as usize];
                if crate::queries::generation::executable(&parsed.value, *item)
                    && engine.lowers_at_check(&source.definition, lowering)
                {
                    diagnostics.extend(engine.lower_to_tir(&source.definition)?.diagnostics);
                }
            }
            // CT-5: each instance a body selects is generated and checked here.
            let roots = parsed
                .value
                .roots
                .iter()
                .map(|id| parsed.value.tree.items[id.0 as usize].definition.clone());
            match engine.select_generated_instances(roots) {
                Ok(instances) => {
                    for instance in instances {
                        diagnostics.extend(engine.lower_to_tir(&instance)?.diagnostics);
                    }
                }
                Err(QueryError::Diagnostics(errors)) => diagnostics.extend(errors),
                Err(error) => return Err(error),
            }
            engine.prepare_staged_instances()?;
        }
        Ok(())
    })();
    let absent = if engine.adapters.checker.is_none() {
        Some(QueryKind::Check)
    } else if engine.adapters.modes.is_none() {
        Some(QueryKind::Modes)
    } else if engine.adapters.lowering.is_none() {
        Some(QueryKind::Lower)
    } else {
        None
    };
    let incomplete = match result {
        Ok(()) => absent,
        Err(QueryError::Diagnostics(errors)) => {
            diagnostics.extend(errors);
            absent
        }
        Err(QueryError::NotImplemented) => Some(
            engine
                .unavailable
                .or(engine.last_phase)
                .unwrap_or(QueryKind::Check),
        ),
        Err(error) => return Err(error),
    };
    let completion = if let Some(phase) = incomplete {
        engine.unavailable = Some(phase);
        CheckCompletion::Incomplete { phase }
    } else {
        CheckCompletion::Complete
    };
    Ok(CheckOutput {
        sources: engine.sources.clone(),
        diagnostics: select_diagnostics(diagnostics, Some(&engine.sources)),
        completion,
        unsafe_ffi: vec![],
    })
}
/// Compatibility helper for execution: only a complete, error-free check may run.
pub fn check_snapshot(
    engine: &mut QueryEngine,
    file: FileId,
    context: &FileContext,
) -> Result<Vec<Diagnostic>, QueryError> {
    let output = check_progress(engine, file, context)?;
    if crate::queries::has_errors(&output.diagnostics) {
        return Err(QueryError::Diagnostics(output.diagnostics));
    }
    if output.completion != CheckCompletion::Complete {
        return Err(QueryError::NotImplemented);
    }
    Ok(output.diagnostics)
}
pub fn format_snapshot(
    engine: &mut QueryEngine,
    file: FileId,
    context: &FileContext,
) -> Result<String, QueryError> {
    let parsed = engine.parse_file(file, context)?;
    let Some(formatter) = engine.adapters.formatter.as_ref() else {
        engine.unavailable = Some(QueryKind::Format);
        return Err(QueryError::NotImplemented);
    };
    let result = formatter.format_query(&parsed.value, &engine.sources);
    if matches!(result, Err(QueryError::NotImplemented)) {
        engine.unavailable = Some(QueryKind::Format);
    }
    result
}
/// Run `f` over the provider interpretation enters every definition through.
/// Under `closure_assertions` that provider refuses any definition outside
/// `entry`'s semantic closure: the program a run identity and `--safe` judge
/// (CAP-6) must hold everything a run executes, finalizers, function values and
/// generated instances included.
fn closed<R>(
    engine: &mut QueryEngine,
    entry: &DefPath,
    f: impl FnOnce(&mut dyn TirProvider) -> Result<R, RuntimeError>,
) -> Result<R, RuntimeError> {
    if !engine.closure_assertions {
        return f(engine);
    }
    let closure = engine
        .semantic_program([entry.clone()])
        .map_err(RuntimeError::Query)?
        .definitions;
    f(&mut Closed {
        engine,
        entry,
        closure,
    })
}
struct Closed<'a, P: TirProvider> {
    engine: &'a mut P,
    entry: &'a DefPath,
    closure: std::collections::BTreeSet<DefPath>,
}
impl<P: TirProvider> TirProvider for Closed<'_, P> {
    fn definition(&mut self, path: &DefPath) -> Result<hirsel_tir::DefinitionRef, QueryError> {
        if !self.closure.contains(path) {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "semantic closure".into(),
                message: format!(
                    "interpretation entered {path}, outside the semantic closure of {}",
                    self.entry
                ),
            }));
        }
        TirProvider::definition(self.engine, path)
    }
    fn constant(&mut self, path: &DefPath) -> Result<hirsel_tir::MaterializedRef, QueryError> {
        TirProvider::constant(self.engine, path)
    }
    fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> Result<hirsel_tir::MaterializedRef, QueryError> {
        TirProvider::constant_instance(self.engine, path, arguments)
    }
    fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
        TirProvider::constant_definitions(self.engine)
    }
    fn instance_body(
        &mut self,
        path: &DefPath,
        types: &hirsel_types::TypeStore,
        arguments: &[hirsel_types::GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        TirProvider::instance_body(self.engine, path, types, arguments)
    }
}
/// Execute a checked program with explicitly simulated capabilities.
pub fn run_snapshot(
    engine: &mut QueryEngine,
    config: &RunConfiguration,
    simulation: hirsel_interp::library::SimulationConfig,
    stdout: &mut dyn Write,
) -> Result<RunOutcome, ToolError> {
    let signature = engine
        .item_signature(&config.entry)
        .map_err(|error| match error {
            // A program without its entry item has nothing to launch.
            QueryError::NotImplemented => ToolError::Runtime(format!(
                "launch: the program declares no entry `pub fn {}`",
                config.entry.0.name
            )),
            error => ToolError::Query {
                error,
                phase: Some(QueryKind::Signature),
                sources: engine.sources.clone(),
            },
        })?
        .value;
    let runtime = engine.runtime_types().map_err(|error| ToolError::Query {
        error,
        phase: Some(QueryKind::Signature),
        sources: engine.sources.clone(),
    })?;
    let Some(mut interpreter) = engine.adapters.interpreter.take() else {
        return Err(ToolError::Query {
            error: QueryError::NotImplemented,
            phase: Some(QueryKind::Interpret),
            sources: engine.sources.clone(),
        });
    };
    interpreter.prepare(signature, runtime, simulation);
    let sources = engine.sources.clone();
    let result = closed(engine, &config.entry, |definitions| {
        interpreter.run_query(config, definitions, stdout, &sources)
    })
    .map_err(|error| match error {
        RuntimeError::Query(error) => ToolError::Query {
            error,
            phase: Some(QueryKind::Interpret),
            sources: engine.sources.clone(),
        },
        RuntimeError::Io(error) => ToolError::Io(error),
        RuntimeError::Tool(message) => ToolError::Runtime(message),
    });
    engine.adapters.interpreter = Some(interpreter);
    result
}
/// Execute once and retain values, complete failures, effects and final simulation state.
pub fn run_observed_snapshot(
    engine: &mut QueryEngine,
    config: &RunConfiguration,
    simulation: SimulationConfig,
) -> Result<ObservedExecution, ToolError> {
    run_observed(engine, config, simulation, false)
}
/// As `run_observed_snapshot`, under checked execution: the same observation,
/// or `RunOutcome::Trapped` at the first memory-model violation.
pub fn run_observed_checked(
    engine: &mut QueryEngine,
    config: &RunConfiguration,
    simulation: SimulationConfig,
) -> Result<ObservedExecution, ToolError> {
    run_observed(engine, config, simulation, true)
}
fn run_observed(
    engine: &mut QueryEngine,
    config: &RunConfiguration,
    simulation: SimulationConfig,
    checked: bool,
) -> Result<ObservedExecution, ToolError> {
    let error_sources = engine.sources.clone();
    let query_error = |error| ToolError::Query {
        error,
        phase: Some(QueryKind::Signature),
        sources: error_sources.clone(),
    };
    let signature = engine
        .item_signature(&config.entry)
        .map_err(query_error)?
        .value;
    let runtime = if checked {
        engine.checked_runtime_types()
    } else {
        engine.runtime_types()
    }
    .map_err(query_error)?;
    let Some(mut interpreter) = engine.adapters.interpreter.take() else {
        return Err(ToolError::Query {
            error: QueryError::NotImplemented,
            phase: Some(QueryKind::Interpret),
            sources: engine.sources.clone(),
        });
    };
    interpreter.prepare(signature, runtime, simulation);
    let sources = engine.sources.clone();
    let result = closed(engine, &config.entry, |definitions| {
        interpreter.run_observed_query(config, definitions, &sources)
    })
    .map_err(|error| match error {
        RuntimeError::Query(error) => ToolError::Query {
            error,
            phase: Some(QueryKind::Interpret),
            sources: engine.sources.clone(),
        },
        RuntimeError::Io(error) => ToolError::Io(error),
        RuntimeError::Tool(message) => ToolError::Runtime(message),
    });
    engine.adapters.interpreter = Some(interpreter);
    result
}
pub fn entry_path(context: &FileContext) -> DefPath {
    DefPath(hirsel_syntax::ItemPath {
        package: context.package.clone(),
        modules: context.modules.clone(),
        name: "main".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Has no definitions: every request that reaches it is refused as missing.
    struct Empty;
    impl TirProvider for Empty {
        fn definition(&mut self, _: &DefPath) -> Result<hirsel_tir::DefinitionRef, QueryError> {
            Err(QueryError::NotImplemented)
        }
        fn constant(&mut self, _: &DefPath) -> Result<hirsel_tir::MaterializedRef, QueryError> {
            Err(QueryError::NotImplemented)
        }
        fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
            Ok(vec![])
        }
    }
    #[test]
    fn interpretation_enters_only_definitions_of_the_closure() {
        let context = default_context();
        let path = |name: &str| {
            let mut path = entry_path(&context);
            path.0.name = name.into();
            path
        };
        let entry = path("main");
        let mut closed = Closed {
            engine: &mut Empty,
            entry: &entry,
            closure: [entry.clone()].into(),
        };
        assert!(matches!(
            closed.definition(&entry),
            Err(QueryError::NotImplemented)
        ));
        let Err(QueryError::Operational(refusal)) = closed.definition(&path("other")) else {
            panic!("a definition outside the closure was entered");
        };
        assert_eq!(refusal.operation, "semantic closure");
    }
}
