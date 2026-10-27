//! Package test discovery and isolated launches over ordinary checked functions.
//! Substring/OR filters adapt Rust libtest filter_tests (third-party/NOTICES.md).
use hirsel_engine::{DefinitionQueries, LoadedProgram, RunOutcome, SimulationConfig, ToolError};
pub use hirsel_interp::library::simulation::SimulationDebug;
pub use hirsel_run_schema::Scheduler;
use hirsel_syntax::{DefPath, FileContext};
use hirsel_types::{SignatureKind, TypeKind};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

mod diff;
pub mod impact;
mod interpreted;

#[derive(Clone, Debug)]
pub struct Test {
    pub definition: DefPath,
    /// A generated ordinary Test entry, in the definition's own module.
    pub entry: DefPath,
    pub seed: u64,
    /// The source file declaring the definition; its snapshots live beside it.
    pub source: PathBuf,
    /// A bench's generated loop entries by iteration count: 0, then 1 to 10^9 by
    /// powers of ten. Empty for a test.
    pub loops: Vec<(u64, DefPath)>,
    /// The launch's scheduler (`inputs.scheduler`); `None` is the default `fifo@1`.
    pub scheduler: Option<Scheduler>,
    pub simulation_debug: SimulationDebug,
}

/// The schedules each selected test runs under.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Schedules {
    /// One run under the default scheduler.
    #[default]
    Default,
    /// One run under this scheduler, as when replaying an explored failure.
    One(Scheduler),
    /// Runs under seeds 0 to `count - 1` of `policy` (whose own seed is unused), stopping
    /// at the first failure.
    Explore { policy: Scheduler, count: u64 },
}

impl Schedules {
    /// The scheduler of each run, in order.
    pub fn runs(&self) -> impl Iterator<Item = Option<Scheduler>> + '_ {
        let count = match self {
            Schedules::Default | Schedules::One(_) => 1,
            Schedules::Explore { count, .. } => *count,
        };
        (0..count).map(move |seed| match self {
            Schedules::Default => None,
            Schedules::One(scheduler) => Some(scheduler.clone()),
            Schedules::Explore { policy, .. } => Some(Scheduler {
                seed,
                ..policy.clone()
            }),
        })
    }
}

/// Run `test` under each of `schedules` until one fails (a Blocked run included, CONC-8):
/// that run's result, else the last run's.
pub fn explore(
    test: &Test,
    schedules: &Schedules,
    mut run: impl FnMut(&Test) -> TestResult,
) -> TestResult {
    let mut last = None;
    for (index, scheduler) in schedules.runs().enumerate() {
        let launch = Test {
            scheduler,
            ..test.clone()
        };
        let mut result = run(&launch);
        result.scheduler = launch.scheduler;
        result.schedules = index as u64 + 1;
        let failed = result.failure.is_some();
        last = Some(result);
        if failed {
            break;
        }
    }
    last.expect("at least one schedule")
}

/// Which marked functions `discover` selects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Selection {
    Tests,
    Benches,
}

impl Selection {
    fn attribute(self) -> &'static str {
        match self {
            Selection::Tests => "test",
            Selection::Benches => "bench",
        }
    }
}

/// A bench's iteration counts: a zero-iteration baseline, then the powers of ten.
pub const BENCH_ITERATIONS: [u64; 11] = [
    0,
    1,
    10,
    100,
    1_000,
    10_000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
    1_000_000_000,
];
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestResult {
    pub name: String,
    pub seed: u64,
    /// None means passed. Failure text is identical across execution engines.
    pub failure: Option<String>,
    /// The test's Test report with log text unescaped, then snapshot diffs.
    pub report: String,
    /// The scheduler of the reported run, when one was selected or explored.
    pub scheduler: Option<Scheduler>,
    /// How many schedules ran.
    pub schedules: u64,
}

/// One bench's calibrated measurement, or its failure.
#[derive(Clone, Debug, PartialEq)]
pub struct BenchResult {
    pub name: String,
    pub seed: u64,
    /// None means the bench ran. Its measurement is then `iterations` calls taking
    /// `nanoseconds` beyond the zero-iteration baseline launch.
    pub failure: Option<String>,
    pub iterations: u64,
    pub nanoseconds: u128,
}

impl BenchResult {
    /// Nanoseconds per call, rounded to one decimal in the report.
    pub fn per_iteration(&self) -> f64 {
        self.nanoseconds as f64 / self.iterations.max(1) as f64
    }
}

/// Go testing.B calibration over `BENCH_ITERATIONS`: one call to warm up, the
/// zero-iteration launch as the baseline, then runs with growing iteration counts until
/// one spends at least `target` beyond the baseline (or the largest count ran). Each next
/// count is the least power of ten predicted, from the last run's time per call, to take
/// 1.2 times the target: at least ten and at most a hundred times the last, so a launch
/// too quick to time cannot leap to an enormous count. `run(index)` launches loop
/// entry `index` and returns its wall time or the bench's failure.
pub fn calibrate(
    target: Duration,
    mut run: impl FnMut(usize) -> Result<Duration, String>,
) -> Result<(u64, u128), String> {
    run(1)?;
    let baseline = run(0)?;
    let mut index = 1;
    loop {
        let work = run(index)?.saturating_sub(baseline).as_nanos();
        let count = BENCH_ITERATIONS[index];
        if work >= target.as_nanos() || index + 1 == BENCH_ITERATIONS.len() {
            return Ok((count, work));
        }
        let per_call = (work / u128::from(count)).max(1);
        let wanted = target.as_nanos() * 6 / 5 / per_call;
        index = (index + 1..BENCH_ITERATIONS.len().min(index + 3))
            .find(|&next| u128::from(BENCH_ITERATIONS[next]) >= wanted)
            .unwrap_or(BENCH_ITERATIONS.len().min(index + 3) - 1);
    }
}

impl Test {
    pub fn bench_result(&self, measured: Result<(u64, u128), String>) -> BenchResult {
        let (failure, iterations, nanoseconds) = match measured {
            Ok((iterations, nanoseconds)) => (None, iterations, nanoseconds),
            Err(failure) => (Some(failure), 0, 0),
        };
        BenchResult {
            name: self.definition.to_string(),
            seed: self.seed,
            failure,
            iterations,
            nanoseconds,
        }
    }

    /// `snapshots/<file stem>.<function>.<name>.snap`, relative to the source's directory.
    pub fn snapshot_file(&self, name: &str) -> PathBuf {
        let stem = self
            .source
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        Path::new("snapshots").join(format!("{stem}.{}.{name}.snap", self.definition.0.name))
    }

    /// The result of one finished run: its own failure, then the snapshots its Test report
    /// recorded, compared with their committed files or, with `bless`, written to them.
    pub fn result(&self, failure: Option<String>, stdout: &[u8], bless: bool) -> TestResult {
        let mut failures: Vec<String> = failure.into_iter().collect();
        let mut report = String::new();
        let mut diffs = String::new();
        let mut unaccepted = false;
        let mut recorded = BTreeSet::new();
        let directory = self.source.parent().unwrap_or(Path::new("")).to_path_buf();
        for line in String::from_utf8_lossy(stdout).lines() {
            if let Some(text) = line.strip_prefix("test log: ") {
                report.push_str(&unescape(text));
                report.push('\n');
                continue;
            }
            let Some((name, text)) = line
                .strip_prefix("test snapshot ")
                .and_then(|rest| rest.split_once(": "))
            else {
                report.push_str(line);
                report.push('\n');
                continue;
            };
            let text = unescape(text);
            if !recorded.insert(name.to_owned()) {
                failures.push(format!("snapshot {name} recorded twice"));
                continue;
            }
            let file = self.snapshot_file(name);
            let path = directory.join(&file);
            let committed = std::fs::read_to_string(&path).ok();
            if committed.as_deref() == Some(text.as_str()) {
                continue;
            }
            let shown = file.to_string_lossy().replace('\\', "/");
            if bless {
                let written = std::fs::create_dir_all(path.parent().unwrap())
                    .and_then(|()| std::fs::write(&path, &text));
                if let Err(error) = written {
                    failures.push(format!("snapshot {name}: cannot write {shown}: {error}"));
                }
                continue;
            }
            unaccepted = true;
            failures.push(match committed {
                Some(_) => format!("snapshot {name} differs from {shown}"),
                None => format!("snapshot {name} has no committed {shown}"),
            });
            let old = committed.unwrap_or_default();
            if let Some(diff) = diff::unified(&shown, &old, "actual", &text) {
                diffs.push_str(&diff);
            }
        }
        report.push_str(&diffs);
        if unaccepted {
            report.push_str("run `hirsel test --bless` to accept the actual snapshots\n");
        }
        TestResult {
            name: self.definition.to_string(),
            seed: self.seed,
            failure: (!failures.is_empty()).then(|| failures.join("; ")),
            report,
            scheduler: self.scheduler.clone(),
            schedules: 1,
        }
    }
}

/// The inverse of `hirsel_kernels::report::escape`, written independently of it.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('u') => {
                let digits: String = chars.by_ref().take_while(|&c| c != '}').collect();
                match digits
                    .strip_prefix('{')
                    .and_then(|d| u32::from_str_radix(d, 16).ok())
                    .and_then(char::from_u32)
                {
                    Some(c) => out.push(c),
                    None => out.push_str(&format!("\\u{digits}}}")),
                }
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Seeds use the first eight SHA-256 bytes of the UTF-8 stable definition path,
/// little endian. Filtering and moving a definition between files do not change it.
pub fn seed(definition: &DefPath) -> u64 {
    let digest = Sha256::digest(definition.to_string().as_bytes());
    u64::from_le_bytes(digest[..8].try_into().unwrap())
}

/// Check the whole package before selecting tests. Dependencies are never selected.
/// Generated entries adapt unit/private definitions to the existing launch ABI;
/// they are checked and lowered normally and never written into source files.
pub fn discover(
    program: &mut LoadedProgram,
    filters: &[String],
    override_seed: Option<u64>,
    selection: Selection,
) -> Result<Vec<Test>, ToolError> {
    let check = program
        .check(hirsel_engine::LoweringPolicy::Reachable)
        .map_err(|e| program.error(e))?;
    if check
        .diagnostics
        .iter()
        .any(|d| d.severity == hirsel_diag::Severity::Error)
    {
        return Err(program.error(hirsel_engine::QueryError::Diagnostics(check.diagnostics)));
    }
    if check.completion != hirsel_engine::CheckCompletion::Complete {
        return Err(program.error(hirsel_engine::QueryError::NotImplemented));
    }
    plan_entries(
        program,
        filters,
        override_seed,
        selection,
        |program, source, entry| {
            let file = program
                .engine
                .sources
                .add(source.name, source.text.to_string())
                .ok_or(ToolError::SourceTooLarge)?;
            let parsed = program
                .engine
                .parse_file(file, &source.context)
                .map_err(|error| program.error(error))?;
            if !parsed.diagnostics.is_empty() {
                return Err(
                    program.error(hirsel_engine::QueryError::Diagnostics(parsed.diagnostics))
                );
            }
            program
                .engine
                .lower_to_tir(entry)
                .map_err(|error| program.error(error))?;
            Ok(())
        },
    )
}

/// Discovery observes one immutable source namespace and accepts only the
/// ordinary admitted Check. Wrapper text is an input to the later compilation;
/// finding tests never checks or lowers a second mutable program.
pub(crate) struct TestDiscovery {
    pub inputs: hirsel_engine::loader_inputs::FrozenInputs,
    pub tests: Vec<Test>,
    pub sources: Vec<crate::requests::NativeSourceText>,
    source_paths: Vec<PathBuf>,
}

pub(crate) fn discover_request(
    path: &Path,
    filters: &[String],
    override_seed: Option<u64>,
    selection: Selection,
    verify: hirsel_engine::Verify,
) -> Result<TestDiscovery, ToolError> {
    discover_with_selector(path, filters, override_seed, selection, verify)
        .map(|(discovery, _)| discovery)
}

fn discover_with_selector(
    path: &Path,
    filters: &[String],
    override_seed: Option<u64>,
    selection: Selection,
    verify: hirsel_engine::Verify,
) -> Result<(TestDiscovery, crate::requests::CompilerRequests), ToolError> {
    use crate::requests::{CompileOperation, CompileRequest, CompileResult, CompilerRequests};
    use hirsel_engine::{
        requests::{OutputReadiness, RequestEngine, observe_inputs},
        runtime::QueryRuntime,
    };
    let compiler = crate::COMPILER.with_verify(verify);
    let mut selector = CompilerRequests::for_host(compiler.clone(), compiler.configuration());
    let compiler = selector.compiler().clone();
    let publication = selector
        .config()
        .store
        .local_product_store()
        .map(|store| store.request());
    let (program, inputs) = observe_inputs(
        selector.input_limits(),
        selector.capture_attempts(),
        |inputs| compiler.load_test_package_inputs(path, inputs),
    )
    .map_err(|_| ToolError::Runtime("test package inputs changed during capture".into()))?;
    let mut program = program?;
    let mut selected = selector.select(&CompileRequest {
        path: path.to_owned(),
        inputs: inputs.clone(),
        operation: CompileOperation::Check {
            lowering: hirsel_engine::LoweringPolicy::Reachable,
        },
    })?;
    let completed = {
        let mut runtime = selected
            .runtime
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let request = runtime.request(&selected.roots);
        runtime.wait(request)
    }
    .map_err(|reference| {
        selected.output.failed(reference).unwrap_or_else(|| {
            ToolError::Runtime("test discovery Check diagnostics unavailable".into())
        })
    })?;
    let output = selected.output.ready(&completed)?;
    let CompileResult::Check(checked) = &output.result else {
        return Err(ToolError::Runtime(
            "test discovery completed another operation".into(),
        ));
    };
    if checked
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == hirsel_diag::Severity::Error)
    {
        return Err(ToolError::Query {
            error: hirsel_engine::QueryError::Diagnostics(checked.diagnostics.clone()),
            phase: None,
            sources: checked.sources.clone(),
        });
    }
    if let hirsel_engine::CheckCompletion::Incomplete { phase } = checked.completion {
        return Err(ToolError::Query {
            error: hirsel_engine::QueryError::NotImplemented,
            phase: Some(phase),
            sources: checked.sources.clone(),
        });
    }
    if let Some(publication) = publication {
        let _ = publication.finish();
    }
    let mut sources = Vec::new();
    let mut source_paths = Vec::new();
    let tests = plan_entries(
        &mut program,
        filters,
        override_seed,
        selection,
        |program, mut source, _| {
            let file = program
                .files
                .iter()
                .find(|(_, context)| *context == source.context)
                .and_then(|(file, _)| program.engine.sources.file(*file))
                .ok_or_else(|| ToolError::Runtime("test wrapper source is unavailable".into()))?;
            source_paths.push(file.path.clone());
            source.name = format!("native-testing/{}.hir", source.name);
            sources.push(source);
            Ok(())
        },
    )?;
    Ok((
        TestDiscovery {
            inputs,
            tests,
            sources,
            source_paths,
        },
        selector,
    ))
}

fn plan_entries(
    program: &mut LoadedProgram,
    filters: &[String],
    override_seed: Option<u64>,
    selection: Selection,
    mut install: impl FnMut(
        &mut LoadedProgram,
        crate::requests::NativeSourceText,
        &DefPath,
    ) -> Result<(), ToolError>,
) -> Result<Vec<Test>, ToolError> {
    let mut definitions = Vec::new();
    let mut sources = std::collections::BTreeMap::new();
    let mut names = BTreeSet::new();
    for (file, context) in program.files.clone() {
        if context.package != program.entry.0.package {
            continue;
        }
        for (index, module) in context.modules.iter().enumerate() {
            names.insert(DefPath(hirsel_syntax::ItemPath {
                package: context.package.clone(),
                modules: context.modules[..index].to_vec(),
                name: module.clone(),
            }));
        }
        let parsed = program
            .engine
            .parse_file(file, &context)
            .map_err(|e| program.error(e))?;
        for syntax in &parsed.value.items {
            let item = &syntax.item;
            names.insert(item.definition.clone());
            if item
                .attributes
                .iter()
                .any(|a| a.name.as_str() == selection.attribute())
                && (filters.is_empty()
                    || filters
                        .iter()
                        .any(|f| item.definition.to_string().contains(f)))
            {
                definitions.push(item.definition.clone());
                if let Some(source) = program.engine.sources.file(file) {
                    sources.insert(item.definition.clone(), source.path.clone());
                }
            }
        }
    }
    definitions.sort();
    let mut tests = Vec::new();
    for definition in definitions {
        let signature = program
            .engine
            .item_signature(&definition)
            .map_err(|e| program.error(e))?
            .value;
        let SignatureKind::Function(f) = &signature.kind else {
            unreachable!("checked @test")
        };
        let unit = matches!(program.engine.types().get(f.result).map(|t| &t.kind), Some(TypeKind::Tuple(fields)) if fields.is_empty());
        let mut generate = |iterations: Option<u64>| -> Result<DefPath, ToolError> {
            let prefix = if iterations.is_some() {
                "bench"
            } else {
                "test"
            };
            let mut entry = definition.clone();
            let mut ordinal = 0;
            let mut context_name = definition.clone();
            loop {
                entry.0.name = format!("hirsel_{prefix}_entry_{ordinal}");
                context_name.0.name = format!("hirsel_{prefix}_context_{ordinal}");
                if !names.contains(&entry) && !names.contains(&context_name) {
                    names.insert(entry.clone());
                    break;
                }
                ordinal += 1;
            }
            let call = format!(
                "{}({})",
                definition,
                if f.parameters.is_empty() {
                    ""
                } else {
                    &context_name.0.name
                }
            );
            // Unit calls keep statement cleanup. Match consumes either Result payload
            // before translating its tag to the ordinary launch ABI.
            let body = match (unit, iterations) {
                (true, None) => format!("  {call}\n  Ok(())\n"),
                (false, None) => {
                    format!("  match {call} {{ Ok(_) => Ok(()), Error(_) => Error(()) }}\n")
                }
                // A bench loop calls the function `count` times; its first Error ends it.
                (unit, Some(count)) => format!(
                    "  let mut hirsel_bench_index: u64 = 0\n  while hirsel_bench_index < {count} {{\n    {}\n    hirsel_bench_index += 1\n  }}\n  Ok(())\n",
                    if unit {
                        call
                    } else {
                        format!("match {call} {{ Ok(_) => {{}}, Error(_) => return Error(()) }}")
                    }
                ),
            };
            let source = format!(
                "@track_caller\npub fn {}({}: core@1/Test) -> core@1/Result[(), ()] {{\n{body}}}\n",
                entry.0.name, context_name.0.name
            );
            install(
                program,
                crate::requests::NativeSourceText {
                    name: match iterations {
                        Some(count) => format!("<bench entry {definition} x{count}>"),
                        None => format!("<test entry {definition}>"),
                    },
                    context: FileContext {
                        package: definition.0.package.clone(),
                        modules: definition.0.modules.clone(),
                    },
                    text: source.into(),
                },
                &entry,
            )?;
            Ok(entry)
        };
        let (entry, loops) = match selection {
            Selection::Tests => (generate(None)?, vec![]),
            Selection::Benches => {
                let loops = BENCH_ITERATIONS
                    .iter()
                    .map(|&count| Ok((count, generate(Some(count))?)))
                    .collect::<Result<Vec<_>, ToolError>>()?;
                (loops[1].1.clone(), loops)
            }
        };
        tests.push(Test {
            seed: override_seed.unwrap_or_else(|| seed(&definition)),
            source: sources.remove(&definition).unwrap_or_default(),
            definition,
            entry,
            loops,
            scheduler: None,
            simulation_debug: Default::default(),
        });
    }
    Ok(tests)
}

pub fn run_interpreted(
    program: &mut LoadedProgram,
    tests: &[Test],
    checked: bool,
    bless: bool,
    schedules: &Schedules,
) -> Vec<TestResult> {
    tests
        .iter()
        .map(|test| {
            explore(test, schedules, |test| {
                let (failure, stdout, races) = match execute(program, test, checked, 0) {
                    Ok(execution) => {
                        let stdout = match &execution.observation {
                            Some(observation) => observation.output.clone(),
                            None => execution.tooling_output,
                        };
                        let failure = match execution.outcome {
                            Ok(outcome) => outcome_failure(&outcome, &program.engine.sources),
                            Err(error) => Some(format!("runtime error: {error:?}")),
                        };
                        (failure, stdout, execution.race_candidates)
                    }
                    Err(error) => (Some(format!("runtime error: {error}")), vec![], vec![]),
                };
                let mut result = test.result(failure, &stdout, bless);
                if result.failure.is_some() {
                    for candidate in races {
                        result.report.push_str(&candidate.to_string());
                    }
                }
                result
            })
        })
        .collect()
}

/// Retain the complete observation for conformance's checked-execution witness.
pub fn execute(
    program: &mut LoadedProgram,
    test: &Test,
    checked: bool,
    hash_seed: u64,
) -> Result<hirsel_engine::ObservedExecution, ToolError> {
    let config = hirsel_tir::RunConfiguration {
        entry: test.entry.clone(),
        hash_seed,
        inputs: vec![],
    };
    let simulation = SimulationConfig {
        grants: Default::default(),
        test_seed: test.seed,
        scheduler: test.scheduler.clone(),
        debug: test.simulation_debug,
        ..Default::default()
    };
    let run = if checked {
        hirsel_engine::run_observed_checked
    } else {
        hirsel_engine::run_observed_snapshot
    };
    run(&mut program.engine, &config, simulation)
}

pub(crate) fn outcome_failure(
    outcome: &RunOutcome,
    sources: &hirsel_span::SourceMap,
) -> Option<String> {
    match outcome {
        RunOutcome::Returned(hirsel_run_schema::Returned::Ok) => None,
        RunOutcome::Returned(hirsel_run_schema::Returned::Error) => Some("returned Error".into()),
        RunOutcome::Returned(hirsel_run_schema::Returned::ExitCode(code)) => {
            Some(format!("returned exit code {code}"))
        }
        RunOutcome::Blocked(waits) => {
            let mut report = String::from("runtime failure: blocked");
            for wait in waits {
                report.push_str(&format!("\n  task {:?}: {}", wait.spawn_path, wait.kind));
            }
            Some(report)
        }
        RunOutcome::Failed {
            primary:
                hirsel_run_schema::Failure {
                    rule,
                    site,
                    operation,
                },
            ..
        } => {
            // Test and bench results are reports against this source snapshot.
            // Resolve canonical outcome sites just as ordinary CLI diagnostics
            // do, retaining logical paths in the outcome itself.
            let physical = |site: &hirsel_run_schema::Site| {
                let mut site = site.clone();
                if let Some(file) = sources.anchors().find_map(|(_, file, _)| {
                    let file = sources.file(file)?;
                    (file.logical_path().to_string_lossy() == site.file).then_some(file)
                }) {
                    site.file = file.path.to_string_lossy().into_owned();
                }
                site
            };
            let site = physical(site);
            let operation = operation.as_ref().map(physical);
            Some(site.report(rule.as_str(), false, operation.as_ref()))
        }
        outcome => Some(format!("runtime failure: {outcome:?}")),
    }
}

/// Interpreted tests run under an explicit step budget; None is unbounded.
pub fn run_path(
    path: &Path,
    filters: &[String],
    override_seed: Option<u64>,
    bless: bool,
    max_steps: Option<u64>,
    schedules: &Schedules,
    verify: hirsel_engine::Verify,
) -> Result<Vec<TestResult>, ToolError> {
    run_path_with_simulation_debug(
        path,
        filters,
        override_seed,
        bless,
        max_steps,
        schedules,
        SimulationTestOptions {
            verify,
            debug: SimulationDebug::default(),
        },
    )
}

/// Verification and simulator diagnostics for one interpreted test launch.
pub struct SimulationTestOptions {
    pub verify: hirsel_engine::Verify,
    pub debug: SimulationDebug,
}

pub fn run_path_with_simulation_debug(
    path: &Path,
    filters: &[String],
    override_seed: Option<u64>,
    bless: bool,
    max_steps: Option<u64>,
    schedules: &Schedules,
    options: SimulationTestOptions,
) -> Result<Vec<TestResult>, ToolError> {
    let (mut discovery, selector) = discover_with_selector(
        path,
        filters,
        override_seed,
        Selection::Tests,
        options.verify,
    )?;
    for test in &mut discovery.tests {
        test.simulation_debug = options.debug;
    }
    let mut execution = interpreted::Executions::new(path, &discovery, selector, max_steps)?;
    Ok(discovery
        .tests
        .iter()
        .map(|test| {
            explore(test, schedules, |test| {
                let mut result = match execution.run(test) {
                    Ok((input, observed)) => {
                        let stdout = match &observed.observation {
                            Some(observation) => &observation.output,
                            None => &observed.tooling_output,
                        };
                        let failure = match &observed.outcome {
                            Ok(outcome) => outcome_failure(outcome, &input.sources),
                            Err(error) => Some(format!("runtime error: {error:?}")),
                        };
                        let mut result = test.result(failure, stdout, bless);
                        if result.failure.is_some() {
                            for candidate in observed.race_candidates {
                                result.report.push_str(&candidate.to_string());
                            }
                        }
                        result
                    }
                    Err(error) => test.result(Some(format!("runtime error: {error}")), &[], bless),
                };
                result.scheduler = test.scheduler.clone();
                result
            })
        })
        .collect())
}

/// Benches run interpreted, each launch timed around the interpreter's execution.
pub fn bench_path(
    path: &Path,
    filters: &[String],
    override_seed: Option<u64>,
    target: Duration,
    verify: hirsel_engine::Verify,
) -> Result<Vec<BenchResult>, ToolError> {
    let (discovery, selector) =
        discover_with_selector(path, filters, override_seed, Selection::Benches, verify)?;
    let mut execution = interpreted::Executions::new(path, &discovery, selector, None)?;
    Ok(discovery
        .tests
        .iter()
        .map(|bench| {
            let measured = calibrate(target, |index| {
                let launch = Test {
                    entry: bench.loops[index].1.clone(),
                    ..bench.clone()
                };
                // Compilation happens once per entry, before the timed interpreter launch.
                execution
                    .prepare(&launch.entry)
                    .map_err(|e| format!("runtime error: {e}"))?;
                let start = Instant::now();
                let (input, observed) = execution
                    .run(&launch)
                    .map_err(|e| format!("runtime error: {e}"))?;
                let elapsed = start.elapsed();
                match observed.outcome {
                    Ok(outcome) => {
                        outcome_failure(&outcome, &input.sources).map_or(Ok(elapsed), Err)
                    }
                    Err(error) => Err(format!("runtime error: {error:?}")),
                }
            });
            bench.bench_result(measured)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn calibration_grows_by_powers_of_ten_to_the_target() {
        use std::time::Duration;
        // 7 ns per call over a 1000 ns launch baseline.
        let mut runs = Vec::new();
        let measured = super::calibrate(Duration::from_micros(100), |index| {
            runs.push(index);
            Ok(Duration::from_nanos(
                1000 + 7 * super::BENCH_ITERATIONS[index],
            ))
        });
        // Warm-up, baseline, one call, then at most a hundredfold steps toward the
        // predicted 120000 / 7 calls: 100, 10^4, then 10^5.
        assert_eq!(runs, [1, 0, 1, 3, 5, 6]);
        assert_eq!(measured, Ok((100_000, 700_000)));
        let failed = super::calibrate(Duration::from_micros(1), |_| Err("boom".into()));
        assert_eq!(failed, Err("boom".into()));
    }

    #[test]
    fn exploration_runs_seeds_in_order_and_stops_at_the_first_failure() {
        use super::{Scheduler, Schedules, Test, explore};
        let test = Test {
            definition: hirsel_syntax::DefPath::core("probe"),
            entry: hirsel_syntax::DefPath::core("probe"),
            seed: 3,
            source: Default::default(),
            loops: vec![],
            scheduler: None,
            simulation_debug: Default::default(),
        };
        let schedules = Schedules::Explore {
            policy: Scheduler::parse("pct@1:0").unwrap(),
            count: 10,
        };
        let mut ran = Vec::new();
        // Seed 4's run is Blocked (CONC-8), a failure like any other.
        let result = explore(&test, &schedules, |launch| {
            let scheduler = launch.scheduler.clone().unwrap();
            ran.push(scheduler.seed);
            let failure = (scheduler.seed == 4).then(|| "runtime failure: blocked".to_string());
            launch.result(failure, b"", false)
        });
        assert_eq!(ran, [0, 1, 2, 3, 4]);
        assert_eq!(result.scheduler.unwrap().to_string(), "pct@1:4");
        assert_eq!((result.schedules, result.seed), (5, 3));
        let passed = explore(&test, &schedules, |launch| launch.result(None, b"", false));
        assert_eq!((passed.failure, passed.schedules), (None, 10));
        let default = explore(&test, &Schedules::Default, |launch| {
            launch.result(None, b"", false)
        });
        assert_eq!((default.scheduler, default.schedules), (None, 1));
    }

    #[test]
    fn unescape_inverts_the_report_escape() {
        for text in ["", "plain", "a\\b\nc\rd\te\u{0}\u{1f}\u{7f}é界", "\\u{zz}"] {
            let mut escaped = Vec::new();
            hirsel_rt::kernels::report::escape(text, &mut escaped);
            assert_eq!(super::unescape(&String::from_utf8(escaped).unwrap()), text);
        }
    }
}
