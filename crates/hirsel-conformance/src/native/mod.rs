//! The native differential gate: every baseline-passing
//! executable corpus case is run by the heap executor and as native code under
//! a named configuration matrix and compared with the reference observation.
//! Large library evidence can explicitly select a stdout sidecar instead;
//! those cases run only compiled configurations.
//!
//! Promotion is set-wise. B is the executable cases in `conformance/passing.txt`;
//! N is the cases every configuration of an engine agrees on. Every case and
//! configuration that did not agree must be exactly one row of
//! `native-tests/regressions/exclusions.tsv`, naming the configuration, a
//! reason class and a reason; a mismatch's reason fingerprints its observation.
//! A disagreement without a row, a row whose reason changed and a row whose
//! configuration now agrees all fail the gate, so support only grows.
pub mod artifact;
pub mod corpus;
pub mod cranelift;
pub mod diet;
pub mod generate;
mod oracle;
pub mod scratch;

pub use scratch::{runtime_cache, scratch};

use hirsel_driver::native::{
    Configuration, Engine, MATRIX_VERSION, NativeError, OBSERVATION_SCHEMA, Observation, Outcome,
    StageName, Toolchain, differential_in,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const EXCLUSIONS: &str = "native-tests/regressions/exclusions.tsv";

/// One executable corpus, regression or library case.
#[derive(Clone, Debug)]
pub struct Case {
    /// The source path relative to the repository root: the case ID.
    pub id: String,
    pub source: String,
    pub path: Option<PathBuf>,
}

impl Case {
    /// An explicit compiled-only library oracle, or the ordinary interpreter default.
    pub fn expected_stdout(&self) -> Result<Option<Observation>, String> {
        oracle::expected_stdout(&self.id, &self.source, self.path.as_deref())
    }
    /// Run this case with its package graph under a case-specific plan.
    pub fn run(&self, matrix: &[Configuration], toolchain: &Toolchain, dir: &Path) -> CaseResult {
        run_source_path(
            &self.id,
            &self.source,
            self.path.as_deref(),
            matrix,
            toolchain,
            dir,
            false,
        )
    }

    /// `--trace-roots` (S-14): run this case under `matrix` with every engine tracing its
    /// `Shared` roots, so a configuration agrees only when its trace equals the
    /// interpreter's line for line.
    pub fn run_traced(
        &self,
        matrix: &[Configuration],
        toolchain: &Toolchain,
        dir: &Path,
    ) -> CaseResult {
        run_source_path(
            &self.id,
            &self.source,
            self.path.as_deref(),
            matrix,
            toolchain,
            dir,
            true,
        )
    }
}

/// The executable cases the ratchet lists as passing: B of the set rule.
pub fn baseline(root: &Path) -> Result<Vec<Case>, String> {
    let rules = crate::spec_rules(root)?;
    let passing = std::fs::read_to_string(root.join("conformance/passing.txt"))
        .map_err(|e| format!("conformance/passing.txt: {e}"))?;
    let mut cases = Vec::new();
    for id in passing.lines().filter(|l| !l.is_empty()) {
        let source = std::fs::read_to_string(root.join(id)).map_err(|e| format!("{id}: {e}"))?;
        let header = crate::parse_header(Path::new(id), &source, &rules)?;
        if matches!(
            header.expect,
            crate::Expect::Test
                | crate::Expect::Run { .. }
                | crate::Expect::Fail { .. }
                | crate::Expect::Blocked { .. }
        ) {
            cases.push(Case {
                id: id.to_string(),
                path: Some(root.join(id)),
                source,
            });
        }
    }
    Ok(cases)
}

/// The engine class a configuration belongs to; each has its own pass set.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Class {
    Heap,
    Llvm,
    Cranelift,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Self::Heap => "heap",
            Self::Llvm => "llvm",
            Self::Cranelift => "cranelift",
        }
    }
    pub fn of(configuration: &Configuration) -> Self {
        match configuration.engine {
            Engine::Heap { .. } => Self::Heap,
            Engine::Native { .. } => Self::Llvm,
            Engine::Cranelift { .. } => Self::Cranelift,
        }
    }
}

/// Why a case is outside an engine's pass set.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Exclusion {
    /// `m2-unavailable`, `native-unavailable` (a stage reports the feature
    /// unsupported), `compiler-error` (a stage validator refused a pass's
    /// output, or a pass failed) or `known-bug` (the engine ran and disagreed).
    pub class: String,
    pub reason: String,
}

/// `text` with each debug-printed `DefPath(..)` replaced by its definition's
/// name and each run of digits by `N`: table indices, layout numbers and
/// offsets vary with unrelated edits, and a reason must not.
fn stable(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("DefPath(") {
        out.push_str(&rest[..start]);
        let body = &rest[start..];
        let mut depth = 0usize;
        let mut end = body.len();
        for (index, c) in body.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = index + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        let path = &body[..end];
        let name = path
            .rfind("name: \"")
            .map(|at| &path[at + 7..])
            .and_then(|tail| tail.split('"').next())
            .unwrap_or("?");
        out.push_str(name);
        rest = &body[end..];
    }
    out.push_str(rest);
    let mut normal = String::new();
    let mut digits = false;
    for c in out.chars() {
        if c.is_ascii_digit() {
            if !digits {
                normal.push('N');
            }
            digits = true;
        } else {
            digits = false;
            normal.push(c);
        }
    }
    normal
}

/// The stable part of a tool error.
fn reason(error: &NativeError) -> String {
    let message = &error.message;
    let quoted = |after: &str| {
        let start = message.find(after)? + after.len();
        let end = message[start..].find('"')? + start;
        Some(message[start..end].to_string())
    };
    let detail = if let Some(owner) = quoted("MissingOwner(\"") {
        format!("no native layout for {owner}")
    } else if let Some(name) = quoted("no native entry for operation \"") {
        format!("operation {name} has no native entry")
    } else if let Some(what) = quoted("Unsupported(\"") {
        stable(&what)
    } else if error.validator {
        let kind: String = message
            .chars()
            .take_while(|c| *c != '(' && *c != ' ')
            .collect();
        format!("validator: {kind}")
    } else {
        // A heap violation repeats its kind and ends with its site; an
        // emission error starts with its unit, which the error already names.
        let text = stable(message);
        let text = match text.split_once(": ") {
            Some((unit, tail)) if unit.starts_with("standalone@") => tail.to_string(),
            _ => text,
        };
        let text = if let Some(at) = text.find("MissingDefinition(") {
            // An operation taken as a function value: which one is noise.
            format!("{}MissingDefinition", &text[..at])
        } else {
            text
        };
        let text = text.split(" at ").next().unwrap_or(&text);
        match text.split_once(": ") {
            Some((kind, tail)) if tail.starts_with(kind) => tail.to_string(),
            _ => text.to_string(),
        }
    };
    let detail: String = detail.chars().take(110).collect();
    format!(
        "{}: {}",
        error.stage.name(),
        detail.replace(['\t', '\n'], " ")
    )
}

/// A mismatch's reason: its observation, abbreviated, and an FNV-1a digest
/// of all of it, so a listed wrong answer that changes no longer matches.
fn mismatch_reason(actual: &Observation) -> String {
    let text = actual.render();
    let digest = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    let summary: String = text
        .lines()
        .skip(1)
        .collect::<Vec<_>>()
        .join(" | ")
        .chars()
        .take(90)
        .collect();
    format!("observed {} ({digest:016x})", summary.replace('\t', " "))
}

pub fn exclusion(outcome: &Outcome) -> Option<Exclusion> {
    match outcome {
        Outcome::Agree => None,
        Outcome::Mismatch(actual) => Some(Exclusion {
            class: "known-bug".into(),
            reason: mismatch_reason(actual),
        }),
        Outcome::Unavailable(error) => {
            let text = reason(error);
            let m2 = [
                "(M2)", "Slots", "Tasks", "Channel", "Mutex", "Atomic", "Arena",
            ]
            .iter()
            .any(|m| error.message.contains(m));
            let unsupported = error.message.contains("Unsupported")
                || error.message.contains("the entry contract")
                || error.message.contains("MissingOwner")
                || error.message.contains("no native entry")
                || error.message.contains("does not support");
            let class = if m2 {
                "m2-unavailable"
            } else if error.stage == StageName::Heap && !unsupported {
                "known-bug"
            } else if unsupported {
                "native-unavailable"
            } else {
                "compiler-error"
            };
            Some(Exclusion {
                class: class.into(),
                reason: text,
            })
        }
    }
}

/// One case's row of the matrix.
#[derive(Clone, Debug)]
pub struct CaseResult {
    pub id: String,
    /// The source or oracle could not be prepared as a native snapshot.
    pub snapshot: Option<String>,
    pub outcomes: Vec<(String, Class, Outcome)>,
    /// The expected observation, from interpretation or an explicit library sidecar.
    pub reference: Option<Observation>,
    /// The case's wall time on its job, every configuration included;
    /// `run_cases_with` sets it.
    pub wall: std::time::Duration,
}

/// The configuration an exclusion row names for a case with no native
/// snapshot, which no configuration ran.
pub const SNAPSHOT: &str = "snapshot";

impl CaseResult {
    /// Every configuration of `classes` that did not agree, with why; a case
    /// with no snapshot is the one `snapshot` row.
    pub fn exclusions(&self, classes: &[Class]) -> Vec<(String, Exclusion)> {
        if let Some(reason) = &self.snapshot {
            return vec![(
                SNAPSHOT.into(),
                Exclusion {
                    class: "compiler-error".into(),
                    reason: reason.clone(),
                },
            )];
        }
        self.outcomes
            .iter()
            .filter(|(_, c, _)| classes.contains(c))
            .filter_map(|(name, _, outcome)| Some((name.clone(), exclusion(outcome)?)))
            .collect()
    }

    /// Whether every configuration of `class` agreed.
    pub fn passes(&self, class: Class) -> bool {
        self.exclusions(&[class]).is_empty()
    }
}

struct Prepared {
    input: hirsel_engine::ValidatedNativeInput,
    entry: hirsel_engine::DefPath,
    reference: Observation,
    compiled_only: bool,
}

fn checked(
    engine: &mut hirsel_engine::QueryEngine,
    id: &str,
    source: &str,
    path: Option<&Path>,
) -> Result<Prepared, String> {
    checked_with_config(engine, id, source, path, Default::default(), false)
}

fn checked_with_config(
    engine: &mut hirsel_engine::QueryEngine,
    id: &str,
    source: &str,
    path: Option<&Path>,
    launch: hirsel_driver::native::LaunchInputs,
    trace_roots: bool,
) -> Result<Prepared, String> {
    let expected = oracle::expected_stdout(id, source, path)?;
    let compiled_only = expected.is_some();
    let (output, entry) = if let Some(path) = path.filter(|p| p.with_extension("hir.json").exists())
    {
        let mut program =
            hirsel_driver::load_program_source(path, Some(source)).map_err(|e| e.to_string())?;
        let output = program
            .check(hirsel_engine::LoweringPolicy::Reachable)
            .map_err(|e| format!("{e:?}"))?;
        let entry = program.entry;
        *engine = program.engine;
        (output, entry)
    } else {
        let context = hirsel_engine::default_context();
        let file = engine
            .sources
            .add("case.hir", source.into())
            .ok_or("source too large")?;
        let output =
            hirsel_engine::check_progress(engine, file, &context).map_err(|e| format!("{e:?}"))?;
        (output, crate::selected_entry(source, &context))
    };
    if let Some(d) = output
        .diagnostics
        .iter()
        .find(|d| d.severity == hirsel_diag::Severity::Error)
    {
        return Err(format!("check: {} {}", d.rule, d.message));
    }
    let reference = if let Some(expected) = expected {
        crate::fixture(source)?;
        expected
    } else {
        let config = hirsel_engine::RunConfiguration {
            entry: entry.clone(),
            hash_seed: launch.hash_seed,
            inputs: vec![],
        };
        engine.adapters.interpreter = Some(crate::interpreter(source)?);
        let execution = hirsel_engine::run_observed_snapshot(
            engine,
            &config,
            hirsel_engine::SimulationConfig {
                random_seed: launch.random_seed,
                test_seed: launch.test_seed,
                trace_roots,
                ..crate::simulation(source)?
            },
        )
        .map_err(|e| format!("reference: {e}"))?;
        execution
            .observation
            .as_ref()
            .map(Observation::of_reference)
            .ok_or("reference: no observation")?
    };
    let input = engine.export_native_input().map_err(|e| {
        let text = format!("{e:?}");
        let head = text.split("DefPath").next().unwrap_or(&text).to_string();
        let _ = head;
        format!(
            "export: {}",
            stable(&text).chars().take(110).collect::<String>()
        )
    })?;
    let validated = engine.import_native_input(input).map_err(|e| {
        stable(&format!("import: {e:?}"))
            .chars()
            .take(110)
            .collect::<String>()
    })?;
    Ok(Prepared {
        input: validated,
        entry,
        reference,
        compiled_only,
    })
}

/// Run one source under `matrix`. Must be called on a compiler stack.
pub fn run_source(
    id: &str,
    source: &str,
    matrix: &[Configuration],
    toolchain: &Toolchain,
    dir: &Path,
) -> CaseResult {
    run_source_path(id, source, None, matrix, toolchain, dir, false)
}
fn run_source_path(
    id: &str,
    source: &str,
    path: Option<&Path>,
    matrix: &[Configuration],
    toolchain: &Toolchain,
    dir: &Path,
    trace_roots: bool,
) -> CaseResult {
    if source.lines().any(|line| line.trim() == "// expect: test") {
        return run_test_suite(id, source, path, matrix, toolchain, dir, trace_roots);
    }
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    match checked_with_config(
        &mut engine,
        id,
        source,
        path,
        matrix
            .first()
            .map(|c| c.options.launch.clone())
            .unwrap_or_default(),
        trace_roots,
    )
    .and_then(|prepared| {
        if prepared.compiled_only
            && !matrix
                .iter()
                .any(|c| !matches!(c.engine, Engine::Heap { .. }))
        {
            Err("expected-stdout oracle requires a compiled native configuration".into())
        } else {
            Ok(prepared)
        }
    }) {
        Err(reason) => CaseResult {
            id: id.to_string(),
            snapshot: Some(reason),
            outcomes: Vec::new(),
            reference: None,
            wall: Default::default(),
        },
        Ok(Prepared {
            input: validated,
            entry,
            reference,
            compiled_only,
        }) => {
            let matrix: Vec<_> = matrix
                .iter()
                .filter(|c| !compiled_only || !matches!(c.engine, Engine::Heap { .. }))
                .cloned()
                .collect();
            let outcomes = differential_in(
                &validated,
                &entry,
                &reference,
                &matrix,
                toolchain,
                dir,
                &hirsel_driver::native::Fixture {
                    trace_roots,
                    ..crate::fixture(source).unwrap()
                },
            )
            .into_iter()
            .zip(&matrix)
            .map(|((name, outcome), configuration)| (name, Class::of(configuration), outcome))
            .collect();
            CaseResult {
                id: id.to_string(),
                snapshot: None,
                outcomes,
                reference: Some(reference),
                wall: Default::default(),
            }
        }
    }
}

/// A test corpus case exercises every selected function in every configuration.
fn run_test_suite(
    id: &str,
    source: &str,
    path: Option<&Path>,
    matrix: &[Configuration],
    toolchain: &Toolchain,
    dir: &Path,
    trace_roots: bool,
) -> CaseResult {
    visit_test_suite(
        id,
        source,
        path,
        dir,
        matrix
            .first()
            .map(|c| c.options.launch.clone())
            .unwrap_or_default(),
        trace_roots,
        |validated, test, reference, test_dir| {
            let mut configurations = matrix.to_vec();
            for c in &mut configurations {
                c.options.launch.test_seed = test.seed;
            }
            differential_in(
                validated,
                &test.entry,
                reference,
                &configurations,
                toolchain,
                test_dir,
                &hirsel_driver::native::Fixture {
                    trace_roots,
                    ..Default::default()
                },
            )
            .into_iter()
            .zip(matrix)
            .map(|((_, outcome), c)| (c.name(), Class::of(c), outcome))
            .collect()
        },
    )
}

/// Share package checking, isolated reference launches and outcome aggregation
/// between differential execution and real-Io record/replay.
fn visit_test_suite(
    id: &str,
    source: &str,
    path: Option<&Path>,
    dir: &Path,
    launch: hirsel_driver::native::LaunchInputs,
    trace_roots: bool,
    probe: impl Fn(
        &hirsel_engine::ValidatedNativeInput,
        &hirsel_driver::testing::Test,
        &Observation,
        &Path,
    ) -> Vec<(String, Class, Outcome)>,
) -> CaseResult {
    let run = || -> Result<CaseResult, String> {
        if oracle::expected_stdout(id, source, path)?.is_some() {
            return Err("expected-stdout oracle requires a main entry, not expect: test".into());
        }
        let fallback = dir.join("test-suite.hir");
        if path.is_none() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            std::fs::write(&fallback, source).map_err(|e| e.to_string())?;
        }
        let mut program =
            hirsel_driver::load_program_source(path.unwrap_or(&fallback), Some(source))
                .map_err(|e| e.to_string())?;
        program.engine.adapters.interpreter = Some(crate::interpreter(source)?);
        let tests = hirsel_driver::testing::discover(
            &mut program,
            &[],
            None,
            hirsel_driver::testing::Selection::Tests,
        )
        .map_err(|e| e.to_string())?;
        if tests.is_empty() {
            return Err("expect: test selected no @test functions".into());
        }
        let input = program
            .engine
            .export_native_input()
            .map_err(|e| format!("export: {e:?}"))?;
        let validated = program
            .engine
            .import_native_input(input)
            .map_err(|e| format!("import: {e:?}"))?;
        let mut outcomes: Vec<(String, Class, Outcome)> = Vec::new();
        let mut first_reference = None;
        for (index, test) in tests.iter().enumerate() {
            let config = hirsel_engine::RunConfiguration {
                entry: test.entry.clone(),
                hash_seed: launch.hash_seed,
                inputs: vec![],
            };
            let simulation = hirsel_engine::SimulationConfig {
                grants: Default::default(),
                random_seed: launch.random_seed,
                test_seed: test.seed,
                trace_roots,
                ..Default::default()
            };
            let execution =
                hirsel_engine::run_observed_snapshot(&mut program.engine, &config, simulation)
                    .map_err(|e| e.to_string())?;
            let reference = execution
                .observation
                .as_ref()
                .map(Observation::of_reference)
                .ok_or("test reference has no observation")?;
            if reference.exit_code != Some(0) {
                return Err(format!("{} failed: {reference:?}", test.definition));
            }
            let test_dir = dir.join(format!("test-{index}"));
            std::fs::create_dir_all(&test_dir).map_err(|e| e.to_string())?;
            let results = probe(&validated, test, &reference, &test_dir);
            if outcomes.is_empty() {
                outcomes = results;
            } else {
                for ((_, _, current), (_, _, result)) in outcomes.iter_mut().zip(results) {
                    if matches!(current, Outcome::Agree) {
                        *current = result;
                    }
                }
            }
            if first_reference.is_none() {
                first_reference = Some(reference);
            }
        }
        Ok(CaseResult {
            id: id.into(),
            snapshot: None,
            outcomes,
            reference: first_reference,
            wall: Default::default(),
        })
    };
    run().unwrap_or_else(|reason| CaseResult {
        id: id.into(),
        snapshot: Some(reason),
        outcomes: vec![],
        reference: None,
        wall: Default::default(),
    })
}

/// Run one source's real-Io record and replay (`real_io_replay`). Must be
/// called on a compiler stack.
pub fn replay_source(id: &str, source: &str, toolchain: &Toolchain, dir: &Path) -> CaseResult {
    replay_source_with_config(id, source, None, toolchain, dir, Default::default())
}
/// `path` selects the case's sidecar package graph, as `Case::run` does.
pub fn replay_source_with_config(
    id: &str,
    source: &str,
    path: Option<&Path>,
    toolchain: &Toolchain,
    dir: &Path,
    launch: hirsel_driver::native::LaunchInputs,
) -> CaseResult {
    if source.lines().any(|line| line.trim() == "// expect: test") {
        return visit_test_suite(
            id,
            source,
            path,
            dir,
            launch.clone(),
            false,
            |validated, test, reference, test_dir| {
                let mut inputs = launch.clone();
                inputs.test_seed = test.seed;
                vec![(
                    "x86_64/real-io-record-and-replay".into(),
                    Class::Llvm,
                    hirsel_driver::native::real_io_replay_with_config(
                        validated,
                        &test.entry,
                        reference,
                        toolchain,
                        test_dir,
                        inputs,
                    ),
                )]
            },
        );
    }
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    match checked_with_config(&mut engine, id, source, path, launch.clone(), false) {
        Err(reason) => CaseResult {
            id: id.to_string(),
            snapshot: Some(reason),
            outcomes: Vec::new(),
            reference: None,
            wall: Default::default(),
        },
        Ok(Prepared {
            input: validated,
            entry,
            reference,
            ..
        }) => CaseResult {
            id: id.to_string(),
            snapshot: None,
            outcomes: vec![(
                "x86_64/real-io-record-and-replay".into(),
                Class::Llvm,
                hirsel_driver::native::real_io_replay_in(
                    &validated,
                    &entry,
                    &reference,
                    toolchain,
                    dir,
                    launch,
                    &crate::fixture(source).unwrap(),
                ),
            )],
            reference: Some(reference),
            wall: Default::default(),
        },
    }
}

/// Run one source's runner oracle (`runner_oracle_in`) under `matrix`'s native
/// configurations, cross-replaying tapes under the first. Must be called on a compiler stack.
pub fn oracle_source(
    id: &str,
    source: &str,
    path: Option<&Path>,
    matrix: &[Configuration],
    toolchain: &Toolchain,
    dir: &Path,
) -> CaseResult {
    let launch = matrix
        .first()
        .map(|c| c.options.launch.clone())
        .unwrap_or_default();
    let probe = |validated: &hirsel_engine::ValidatedNativeInput,
                 entry: &hirsel_engine::DefPath,
                 matrix: &[Configuration],
                 dir: &Path| {
        let natives: Vec<_> = matrix
            .iter()
            .filter(|c| !matches!(c.engine, Engine::Heap { .. }))
            .cloned()
            .collect();
        let fixture = crate::fixture(source).unwrap();
        natives
            .iter()
            .enumerate()
            .flat_map(|(index, configuration)| {
                hirsel_driver::native::runner_oracle_in(
                    validated,
                    entry,
                    std::slice::from_ref(configuration),
                    toolchain,
                    dir,
                    &fixture,
                    index == 0,
                )
                .into_iter()
                .map(|(name, outcome)| (name, Class::of(configuration), outcome))
            })
            .collect()
    };
    if source.lines().any(|line| line.trim() == "// expect: test") {
        return visit_test_suite(
            id,
            source,
            path,
            dir,
            launch,
            false,
            |validated, test, _, test_dir| {
                let mut configurations = matrix.to_vec();
                for c in &mut configurations {
                    c.options.launch.test_seed = test.seed;
                }
                probe(validated, &test.entry, &configurations, test_dir)
            },
        );
    }
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    match checked_with_config(&mut engine, id, source, path, launch, false) {
        Err(reason) => CaseResult {
            id: id.to_string(),
            snapshot: Some(reason),
            outcomes: Vec::new(),
            reference: None,
            wall: Default::default(),
        },
        Ok(Prepared {
            input: validated,
            entry,
            reference,
            ..
        }) => CaseResult {
            id: id.to_string(),
            snapshot: None,
            outcomes: probe(&validated, &entry, matrix, dir),
            reference: Some(reference),
            wall: Default::default(),
        },
    }
}

/// Run every case on `jobs` threads, each in its own scratch directory.
pub fn run_cases(
    cases: &[Case],
    matrix: &[Configuration],
    toolchain: &Toolchain,
    scratch: &Path,
    jobs: usize,
) -> Vec<CaseResult> {
    run_cases_with(cases, scratch, jobs, |case, dir| {
        case.run(matrix, toolchain, dir)
    })
}

/// `run_cases` with the per-case run given.
pub fn run_cases_with(
    cases: &[Case],
    scratch: &Path,
    jobs: usize,
    run: impl Fn(&Case, &Path) -> CaseResult + Sync,
) -> Vec<CaseResult> {
    let run = &run;
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for job in 0..jobs.max(1) {
            let (next, results) = (&next, &results);
            scope.spawn(move || {
                let dir = scratch.join(format!("job-{job}"));
                std::fs::create_dir_all(&dir).expect("scratch directory");
                hirsel_engine::with_compiler_stack(move || {
                    loop {
                        let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(case) = cases.get(index) else { break };
                        std::fs::write(dir.join("case.txt"), &case.id)
                            .expect("scratch case identity");
                        let started = std::time::Instant::now();
                        let mut result =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                run(case, &dir)
                            }))
                            .unwrap_or_else(|_| CaseResult {
                                id: case.id.clone(),
                                snapshot: Some("a native stage panicked".into()),
                                outcomes: Vec::new(),
                                reference: None,
                                wall: Default::default(),
                            });
                        result.wall = started.elapsed();
                        results.lock().unwrap().push((index, result));
                    }
                });
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, r)| r).collect()
}

/// The checked-in exclusions: `(case, configuration) -> exclusion`.
pub type Exclusions = BTreeMap<(String, String), Exclusion>;

/// The engine class of a configuration an exclusion row names; `snapshot`
/// belongs to every class.
fn row_class(configuration: &str) -> Option<Option<Class>> {
    if configuration == SNAPSHOT {
        return Some(None);
    }
    artifact::configuration(configuration).map(|c| Some(Class::of(&c)))
}

/// Whether `case` has a row naming its snapshot or any configuration of
/// `class`.
pub fn listed_under(exclusions: &Exclusions, case: &str, class: Class) -> bool {
    exclusions
        .range((case.to_string(), String::new())..)
        .take_while(|((id, _), _)| id == case)
        .any(|((_, configuration), _)| {
            row_class(configuration).is_some_and(|c| c.is_none_or(|c| c == class))
        })
}

/// The checked-in exclusions, each naming a configuration of the current
/// matrix.
pub fn read_exclusions(root: &Path) -> Result<Exclusions, String> {
    let text =
        std::fs::read_to_string(root.join(EXCLUSIONS)).map_err(|e| format!("{EXCLUSIONS}: {e}"))?;
    let exclusions = parse_exclusions(&text)?;
    for (id, configuration) in exclusions.keys() {
        if row_class(configuration).is_none() {
            return Err(format!(
                "{EXCLUSIONS}: {id}: {configuration} is not a configuration of matrix {MATRIX_VERSION}"
            ));
        }
    }
    Ok(exclusions)
}

/// Exclusion rows as `render_exclusions` writes them, sorted and unique.
pub fn parse_exclusions(text: &str) -> Result<Exclusions, String> {
    let mut out = Exclusions::new();
    let mut previous = None;
    for (number, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        let [id, configuration, class, reason] = fields.as_slice() else {
            return Err(format!("{EXCLUSIONS}:{}: expected four fields", number + 1));
        };
        let key = (id.to_string(), configuration.to_string());
        if previous.as_ref().is_some_and(|p| *p >= key) {
            return Err(format!(
                "{EXCLUSIONS}:{}: rows must be sorted and unique",
                number + 1
            ));
        }
        previous = Some(key.clone());
        out.insert(
            key,
            Exclusion {
                class: class.to_string(),
                reason: reason.to_string(),
            },
        );
    }
    Ok(out)
}

pub fn render_exclusions(results: &[CaseResult]) -> String {
    let mut text = String::from(
        "# Baseline-passing executable cases a native configuration does not agree on, with\n\
         # the reason. Each row is an unmet promotion obligation, not a\n\
         # waiver. Regenerate with `native --full --bless` only after reading the diff:\n\
         # a new row is a regression unless its reason is new work.\n\
         # case\tconfiguration\tclass\treason\n",
    );
    let mut rows: Vec<_> = results
        .iter()
        .flat_map(|result| {
            result
                .exclusions(&[Class::Heap, Class::Llvm])
                .into_iter()
                .map(|(configuration, exclusion)| (result.id.clone(), configuration, exclusion))
        })
        .collect();
    rows.sort();
    for (id, configuration, exclusion) in rows {
        text.push_str(&format!(
            "{id}\t{configuration}\t{}\t{}\n",
            exclusion.class, exclusion.reason
        ));
    }
    text
}

/// The set rule's verdict over one run.
#[derive(Debug, Default)]
pub struct Report {
    pub baseline: usize,
    pub configurations: usize,
    /// Cases every configuration of the engine agrees on.
    pub pass: BTreeMap<Class, usize>,
    /// Excluded `(case, configuration)` pairs by class.
    pub excluded: BTreeMap<String, usize>,
    /// Disagreeing `(case, configuration)` pairs with no row, or whose reason changed.
    pub unlisted: Vec<String>,
    /// Exclusion rows whose configuration now agrees, or whose case or
    /// configuration a complete run no longer has.
    pub stale: Vec<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.unlisted.is_empty() && self.stale.is_empty()
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "matrix_version": MATRIX_VERSION,
            "observation_schema": OBSERVATION_SCHEMA,
            "baseline_executable": self.baseline,
            "configurations": self.configurations,
            "heap_pass": self.pass.get(&Class::Heap).copied().unwrap_or(0),
            "llvm_pass": self.pass.get(&Class::Llvm).copied().unwrap_or(0),
            "excluded": self.excluded,
            "baseline_minus_native_unlisted": self.unlisted,
            "stale_exclusions": self.stale,
        })
    }
}

/// Compare a run with the checked-in exclusions. `complete` says the run
/// covered the whole baseline, so rows for cases it did not run are stale.
pub fn judge(
    results: &[CaseResult],
    exclusions: &Exclusions,
    configurations: usize,
    complete: bool,
) -> Report {
    judge_classes(
        results,
        exclusions,
        configurations,
        complete,
        &[Class::Heap, Class::Llvm],
    )
}

/// Judge only the engine classes the selected configuration matrix actually
/// ran: each case and configuration of them against its row.
pub fn judge_classes(
    results: &[CaseResult],
    exclusions: &Exclusions,
    configurations: usize,
    complete: bool,
    classes: &[Class],
) -> Report {
    let mut report = Report {
        baseline: results.len(),
        configurations,
        ..Report::default()
    };
    let mut judged = BTreeSet::new();
    for result in results {
        for &class in classes {
            if result.passes(class) {
                *report.pass.entry(class).or_default() += 1;
            }
        }
        let found: BTreeMap<String, Exclusion> = result.exclusions(classes).into_iter().collect();
        let ran = std::iter::once(SNAPSHOT).chain(
            result
                .outcomes
                .iter()
                .filter(|(_, class, _)| classes.contains(class))
                .map(|(name, _, _)| name.as_str()),
        );
        for configuration in ran {
            let key = (result.id.clone(), configuration.to_string());
            let listed = exclusions.get(&key);
            judged.insert(key);
            match (found.get(configuration), listed) {
                (None, None) => {}
                (None, Some(_)) => report.stale.push(format!(
                    "{} under {configuration}: now agrees; delete its exclusion row",
                    result.id
                )),
                (Some(found), listed) => {
                    *report.excluded.entry(found.class.clone()).or_default() += 1;
                    if listed != Some(found) {
                        report.unlisted.push(format!(
                            "{} under {configuration}: {} ({}){}",
                            result.id,
                            found.reason,
                            found.class,
                            match listed {
                                Some(l) => format!("; listed as {} ({})", l.reason, l.class),
                                None => String::new(),
                            }
                        ));
                    }
                }
            }
        }
    }
    if complete {
        for (id, configuration) in exclusions.keys() {
            if !judged.contains(&(id.clone(), configuration.clone())) {
                report.stale.push(format!(
                    "{id} under {configuration}: not a baseline case and configuration of this run"
                ));
            }
        }
    }
    report
}

/// The versioned case/configuration matrix as TSV: one row per case and
/// configuration, with what the configuration did.
pub fn render_matrix(results: &[CaseResult]) -> String {
    let mut text = format!(
        "# matrix {MATRIX_VERSION} observation {OBSERVATION_SCHEMA}\n# case\tconfiguration\tstatus\tdetail\n"
    );
    for result in results {
        if let Some(reason) = &result.snapshot {
            text.push_str(&format!("{}\tsnapshot\tunavailable\t{reason}\n", result.id));
        }
        for (name, _, outcome) in &result.outcomes {
            let (status, detail) = match outcome {
                Outcome::Agree => ("agree", String::new()),
                Outcome::Mismatch(actual) => ("mismatch", actual.render().replace('\n', " | ")),
                Outcome::Unavailable(error) => ("unavailable", reason(error)),
            };
            text.push_str(&format!("{}\t{name}\t{status}\t{detail}\n", result.id));
        }
    }
    text
}

/// `case<TAB>seconds` for every case, in case order.
pub fn render_walls(results: &[CaseResult]) -> String {
    let mut text = String::from("case\tseconds\n");
    for result in results {
        text.push_str(&format!(
            "{}\t{:.1}\n",
            result.id,
            result.wall.as_secs_f64()
        ));
    }
    text
}
