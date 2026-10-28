use crate::*;
mod front;
pub(crate) use front::{DeadArmUse, FrontProducts};
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
    Launch(hirsel_run_schema::RunRefusal),
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

/// The std shipped with this toolchain. Source-tree builds use their checkout;
/// distributed bundles carry the same tree beside the executable and precheck.
pub fn toolchain_std_manifest() -> Result<std::path::PathBuf, ToolError> {
    toolchain_std_manifest_inputs(&crate::loader_inputs::DiskInputs)
}
pub(crate) fn toolchain_std_manifest_inputs(
    inputs: &dyn crate::loader_inputs::LoaderInputs,
) -> Result<std::path::PathBuf, ToolError> {
    if let Some(manifest) = inputs.toolchain_std_manifest() {
        return Ok(manifest?);
    }
    let executable = std::env::current_exe()?;
    // Buck2 and Cargo executables live below their source tree; installed
    // toolchains put stdlib immediately beside the binary.
    for directory in executable.ancestors().skip(1) {
        let manifest = directory.join("stdlib/std@1/hirsel.json");
        match inputs.metadata(&manifest) {
            Ok(_) => return Ok(inputs.canonicalize(&manifest)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    // Relocated test harnesses retain repository-relative CARGO_MANIFEST_DIR.
    Ok(inputs.canonicalize(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("stdlib/std@1/hirsel.json"),
    )?)
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
pub const SPEC_PARTS: [(&str, &[u8]); 27] = [
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
        "runners.md",
        include_bytes!("../../../docs/spec/runners.md"),
    ),
    ("schema.md", include_bytes!("../../../docs/spec/schema.md")),
    (
        "scoped-transport.md",
        include_bytes!("../../../docs/spec/scoped-transport.md"),
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
    (
        "storage.md",
        include_bytes!("../../../docs/spec/storage.md"),
    ),
    ("syntax.md", include_bytes!("../../../docs/spec/syntax.md")),
    (
        "utf8-streaming.md",
        include_bytes!("../../../docs/spec/utf8-streaming.md"),
    ),
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

pub fn read_source(path: &Path) -> Result<String, ToolError> {
    decode_source(path, std::fs::read(path)?)
}

/// Decode the exact captured bytes with the ordinary source diagnostic policy.
pub(crate) fn decode_source(path: &Path, bytes: Vec<u8>) -> Result<String, ToolError> {
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
            let span =
                hirsel_span::Span::new(sources.file_anchor(file), offset as u32, offset as u32)
                    .unwrap();
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
    let _timing = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::S0);
    engine.index_source(file, context)?;
    let mut diagnostics = vec![];
    for item in engine.source_items(file) {
        engine.work.association_visits += 1;
        for (kind, start, end) in item.facts.associations {
            let phases = engine
                .adapters
                .phases
                .clone()
                .ok_or(QueryError::NotImplemented)?;
            let span = hirsel_span::Span {
                anchor: item.anchor,
                start,
                end,
            };
            let (types, memo) = (engine.types.clone(), engine.memo.clone());
            let mut provider = engine.association_signature_provider();
            let result = hirsel_types::SignatureProvider::signature(
                &mut provider,
                hirsel_syntax::DefId::intern(&item.definition),
            )
            .and_then(|signature| {
                if kind == 0 {
                    phases.validate_format(&types, &memo, &signature, span, &mut provider)
                } else if kind == 1 {
                    phases.validate_serializable_check(
                        &types,
                        &memo,
                        &signature,
                        span,
                        &mut provider,
                    )
                } else {
                    phases.validate_loop_source(&types, &memo, &signature, span, &mut provider)
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
    check_file_lowering(engine, file, context, LoweringPolicy::Reachable)
}
pub(crate) fn check_file_lowering(
    engine: &mut QueryEngine,
    file: FileId,
    context: &FileContext,
    lowering: LoweringPolicy,
) -> Result<CheckOutput, QueryError> {
    let mut outputs = check_files(engine, &[(file, context, lowering)])?;
    let output = outputs.pop().expect("one file's check").1;
    Ok(CheckOutput {
        sources: engine.sources.clone(),
        diagnostics: output.diagnostics,
        completion: output.completion,
        unsafe_ffi: vec![],
    })
}

/// A file's verdict, sharing the program's source map until final output.
pub(crate) struct FileCheckOutput {
    pub diagnostics: Vec<Diagnostic>,
    pub completion: CheckCompletion,
}

/// One file's state through `check_files`.
struct FileCheck {
    parsed: Option<Vec<crate::queries::catalog::SourceItem>>,
    diagnostics: Vec<Diagnostic>,
    /// The first error that stopped the file's steps.
    stopped: Option<QueryError>,
    missing_phase: Option<QueryKind>,
}
impl FileCheck {
    /// The file goes on to the next step: nothing stopped it and it has no error.
    fn clean(&self) -> bool {
        self.stopped.is_none() && !crate::queries::has_errors(&self.diagnostics)
    }
    /// Runs one step of the file unless it is no longer clean; a step's
    /// diagnostics error is its diagnostics, any other error stops the file.
    fn step(
        &mut self,
        engine: &mut QueryEngine,
        step: impl FnOnce(
            &mut QueryEngine,
            &[crate::queries::catalog::SourceItem],
            &mut Vec<Diagnostic>,
        ) -> Result<(), QueryError>,
    ) -> bool {
        if !self.clean() {
            return false;
        }
        let Some(parsed) = &self.parsed else {
            return false;
        };
        match step(engine, parsed, &mut self.diagnostics) {
            Ok(()) => return true,
            Err(QueryError::Diagnostics(errors)) => self.diagnostics.extend(errors),
            Err(error) => {
                self.missing_phase = engine.unavailable.or(engine.last_phase);
                self.stopped = Some(error);
            }
        }
        false
    }
}

/// Publish one ordinary frontier's input-ordered outcomes into its files.
/// Source diagnostics accumulate throughout P; an operational failure stops
/// that file before any later generator request is admitted.
fn mode_frontier(
    engine: &mut QueryEngine,
    checks: &mut [FileCheck],
    pending: &mut Vec<(usize, DefPath)>,
    jobs: hirsel_intern::jobs::Jobs,
) {
    if pending.is_empty() {
        return;
    }
    let (owners, definitions): (Vec<_>, Vec<_>) = std::mem::take(pending).into_iter().unzip();
    for (owner, mode) in owners
        .into_iter()
        .zip(engine.front_definitions(definitions, jobs))
    {
        engine
            .checked_scope
            .front
            .insert(mode.definition.clone(), mode.reads.clone());
        let check = &mut checks[owner];
        if check.stopped.is_some() {
            continue;
        }
        match mode.result {
            Ok(output) => check.diagnostics.extend(output.diagnostics.iter().cloned()),
            Err(QueryError::Diagnostics(errors)) => check.diagnostics.extend(errors),
            Err(error) => {
                check.missing_phase = mode.phase;
                check.stopped = Some(error);
            }
        }
    }
}

/// A program check's schedule over `files` (design §1c): S0 parses every
/// file and checks its associations; P checks and mode-checks every item of
/// every clean file; GEN-5 runs once, over the summaries P published; S1
/// lowers each clean file's compile-time bodies and selects its CT-5
/// instances; P2 lowers the rest under `Reachable`. A file with an error
/// after a step takes no later step. Each file's diagnostics and completion
/// are its own, in `files` order.
///
/// GEN-5 runs after every body is checked, as Go's checker runs `monomorph`
/// once after `processDelayed` (types2/check.go:446-465, mono.go:82) and
/// rustc type-checks every body owner before crate-wide passes
/// (rustc_hir_analysis/src/lib.rs:146-220). Before, GEN-5 ran in the first
/// clean file's step and forced every other definition's check from there.
pub(crate) fn check_files(
    engine: &mut QueryEngine,
    files: &[(FileId, &FileContext, LoweringPolicy)],
) -> Result<Vec<(FileId, FileCheckOutput)>, QueryError> {
    check_files_impl(engine, files, true)
}
pub(crate) fn check_files_impl(
    engine: &mut QueryEngine,
    files: &[(FileId, &FileContext, LoweringPolicy)],
    recipes: bool,
) -> Result<Vec<(FileId, FileCheckOutput)>, QueryError> {
    use hirsel_intern::timings::{Interval, interval};
    engine.checked_scope = Default::default();
    let _reads = engine
        .result_store
        .as_ref()
        .and_then(|store| store.read_scope());
    // Canonical type records are written while queries encode their products,
    // before the per-record writers run. Capacity spills stay private; a
    // completed request explicitly publishes its last bounded subepoch.
    let publication = engine
        .result_store
        .as_ref()
        .and_then(|store| store.local_product_store())
        .map(|store| store.request());
    let s0 = interval(Interval::S0);
    let mut checks: Vec<FileCheck> = files
        .iter()
        .map(|&(file, context, _)| {
            let mut check = FileCheck {
                parsed: None,
                diagnostics: vec![],
                stopped: None,
                missing_phase: None,
            };
            match engine.index_source(file, context) {
                Ok(()) => check.parsed = Some(engine.source_items(file)),
                Err(QueryError::Diagnostics(errors)) => check.diagnostics.extend(errors),
                Err(error) => {
                    check.missing_phase = engine.unavailable.or(engine.last_phase);
                    check.stopped = Some(error);
                }
            }
            check.step(engine, |engine, _, diagnostics| {
                diagnostics.extend(association_diagnostics(engine, file, context)?);
                Ok(())
            });
            check
        })
        .collect();
    drop(s0);
    let p = interval(Interval::Parallel);
    // Keep source order for per-file diagnostics, while the engine publishes
    // worker products in DefPath order. Every clean file contributes its body
    // tasks and distinct-type fact helpers to P, draining before generators.
    let mut planned = vec![];
    for (owner, check) in checks.iter().enumerate().filter(|(_, check)| check.clean()) {
        let Some(parsed) = &check.parsed else {
            continue;
        };
        for item in parsed {
            engine.work.planning_visits += 1;
            planned.push((owner, item.definition.clone(), item.facts.generator));
            if item.facts.fact {
                planned.push((
                    owner,
                    hirsel_check_protocol::fact::fact_path(&item.definition),
                    false,
                ));
            }
        }
    }
    let jobs = engine.jobs();
    let mut pending = vec![];
    for (owner, definition, generates) in planned {
        if generates {
            // A generator executes S1. Publish earlier ordinary outcomes first
            // so an unavailable phase in this file can withhold that execution.
            mode_frontier(engine, &mut checks, &mut pending, jobs);
            if checks[owner].stopped.is_none() {
                pending.push((owner, definition));
                mode_frontier(engine, &mut checks, &mut pending, jobs);
            }
        } else if checks[owner].stopped.is_none() {
            pending.push((owner, definition));
        }
    }
    mode_frontier(engine, &mut checks, &mut pending, jobs);
    drop(p);
    // GEN-5 is the program's: every clean file reports its verdict.
    if checks.iter().any(FileCheck::clean) {
        let verdict = crate::generic_graph::check(engine);
        for check in checks.iter_mut().filter(|c| c.clean()) {
            match &verdict {
                Ok(errors) => check.diagnostics.extend(errors.iter().cloned()),
                Err(QueryError::Diagnostics(errors)) => {
                    check.diagnostics.extend(errors.iter().cloned())
                }
                Err(error) => {
                    check.missing_phase = engine.unavailable.or(engine.last_phase);
                    check.stopped = Some(error.clone());
                }
            }
        }
    }
    let s1 = interval(Interval::S1);
    let mut rests = vec![vec![]; files.len()];
    let mut prepare = vec![false; files.len()];
    for ((check, rest), prepare) in checks.iter_mut().zip(&mut rests).zip(&mut prepare) {
        *prepare = check.step(engine, |engine, parsed, diagnostics| {
            // Every policy reports the diagnostics of the compile-time bodies
            // and CT-5 selection first; only then, and only for an accepted
            // file, does `Reachable` lower the rest, which reports none.
            let comptime: Vec<_>;
            (comptime, *rest) = parsed
                .iter()
                .filter(|item| item.facts.executable)
                .map(|item| item.definition.clone())
                .partition(|definition| {
                    engine.lowers_at_check(definition, LoweringPolicy::Comptime)
                });
            engine.front_lowerings(comptime, jobs, diagnostics)?;
            // CT-5: each instance a body selects is generated and checked here.
            let roots = parsed.iter().map(|item| item.definition.clone());
            match engine.select_generated_instances(roots) {
                Ok(instances) => {
                    engine.front_lowerings(instances.into_iter().collect(), jobs, diagnostics)?;
                }
                Err(QueryError::Diagnostics(errors)) => diagnostics.extend(errors),
                Err(error) => return Err(error),
            }
            Ok(())
        });
    }
    drop(s1);
    let p2 = interval(Interval::Parallel2);
    for (((check, rest), &(_, _, lowering)), prepare) in
        checks.iter_mut().zip(rests).zip(files).zip(prepare)
    {
        let lower_rest = check.clean();
        let completed = check.step(engine, |engine, _, diagnostics| {
            if lowering == LoweringPolicy::Reachable {
                for definition in rest {
                    diagnostics.extend(engine.front_lower(&definition)?);
                }
            }
            Ok(())
        });
        if prepare
            && (!lower_rest || completed)
            && let Err(error) = engine.prepare_staged_instances()
        {
            match error {
                QueryError::Diagnostics(errors) => check.diagnostics.extend(errors),
                error => {
                    check.missing_phase = engine.unavailable.or(engine.last_phase);
                    check.stopped = Some(error);
                }
            }
        }
    }
    drop(p2);
    let absent = if engine.adapters.checker.is_none() {
        Some(QueryKind::Check)
    } else if engine.adapters.modes.is_none() {
        Some(QueryKind::Modes)
    } else if engine.adapters.lowering.is_none() {
        Some(QueryKind::Lower)
    } else {
        None
    };
    let mut outputs = Vec::with_capacity(files.len());
    for (check, &(file, _, _)) in checks.into_iter().zip(files) {
        let incomplete = match check.stopped {
            None => absent,
            Some(QueryError::NotImplemented) => {
                Some(check.missing_phase.unwrap_or(QueryKind::Check))
            }
            Some(error) => return Err(error),
        };
        let completion = if let Some(phase) = incomplete {
            engine.unavailable = Some(phase);
            CheckCompletion::Incomplete { phase }
        } else {
            CheckCompletion::Complete
        };
        outputs.push((
            file,
            FileCheckOutput {
                diagnostics: select_diagnostics(check.diagnostics, Some(&engine.sources)),
                completion,
            },
        ));
    }
    if outputs
        .iter()
        .all(|(_, output)| output.completion == CheckCompletion::Complete)
        && engine.last_comptime_limit.is_none()
    {
        engine.checked_scope.completed = Some(engine.graph.revision());
    }
    if outputs
        .iter()
        .all(|(_, output)| output.completion == CheckCompletion::Complete)
        && let Some(publication) = publication
    {
        let _ = publication.finish();
    }
    if recipes {
        for ((file, context, lowering), (_, output)) in files.iter().zip(&mut outputs) {
            crate::queries::recipes::offer(
                engine,
                *file,
                context,
                *lowering,
                &mut output.diagnostics,
            );
        }
    }
    Ok(outputs)
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
        arguments: &[hirsel_types::GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        TirProvider::instance_body(self.engine, path, arguments)
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
/// Execute a reference fixture with a finite, explicit per-run step bound.
/// Bounds below `hirsel_interp::DEFAULT_STEP_LIMIT` are refused.
/// The result records the requested bound and consumed steps in `step_usage`.
/// The adapter's previous policy is restored even when execution returns an error.
pub fn run_observed_snapshot_bounded(
    engine: &mut QueryEngine,
    config: &RunConfiguration,
    simulation: SimulationConfig,
    step_limit: u64,
) -> Result<ObservedExecution, ToolError> {
    run_observed_bounded(engine, config, simulation, step_limit, false)
}
/// Checked counterpart of `run_observed_snapshot_bounded`, with identical charges.
pub fn run_observed_checked_bounded(
    engine: &mut QueryEngine,
    config: &RunConfiguration,
    simulation: SimulationConfig,
    step_limit: u64,
) -> Result<ObservedExecution, ToolError> {
    run_observed_bounded(engine, config, simulation, step_limit, true)
}
fn run_observed_bounded(
    engine: &mut QueryEngine,
    config: &RunConfiguration,
    simulation: SimulationConfig,
    step_limit: u64,
    checked: bool,
) -> Result<ObservedExecution, ToolError> {
    if step_limit < hirsel_interp::DEFAULT_STEP_LIMIT {
        return Err(ToolError::Runtime(format!(
            "application step bound {step_limit} must be at least {}",
            hirsel_interp::DEFAULT_STEP_LIMIT
        )));
    }
    let interpreter = engine.adapters.interpreter.as_mut().ok_or_else(|| {
        ToolError::Runtime("bounded reference execution requires an interpreter".into())
    })?;
    let previous = interpreter
        .replace_step_limit(Some(step_limit))
        .map_err(|_| {
            ToolError::Runtime("interpreter does not support explicit step limits".into())
        })?;
    let result = run_observed(engine, config, simulation, checked);
    engine
        .adapters
        .interpreter
        .as_mut()
        .expect("run restores interpreter")
        .replace_step_limit(previous)
        .expect("adapter accepted step limit");
    result
}
fn run_observed(
    engine: &mut QueryEngine,
    config: &RunConfiguration,
    mut simulation: SimulationConfig,
    checked: bool,
) -> Result<ObservedExecution, ToolError> {
    let error_sources = engine.sources.clone();
    let query_error = |error| ToolError::Query {
        error,
        phase: Some(QueryKind::Signature),
        sources: error_sources.clone(),
    };
    if !simulation.runners.is_empty() {
        simulation.callees =
            std::sync::Arc::new(crate::callee_facts(engine, &config.entry).map_err(query_error)?);
    }
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
