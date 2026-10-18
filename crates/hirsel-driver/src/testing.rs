//! Package test discovery and isolated launches over ordinary checked functions.
//! Substring/OR filters adapt Rust libtest filter_tests (third-party/NOTICES.md).
use hirsel_engine::{DefinitionQueries, LoadedProgram, RunOutcome, SimulationConfig, ToolError};
use hirsel_syntax::{DefPath, FileContext};
use hirsel_types::{SignatureKind, TypeKind};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};

#[derive(Clone, Debug)]
pub struct Test {
    pub definition: DefPath,
    /// A generated ordinary Test entry, in the definition's own module.
    pub entry: DefPath,
    pub seed: u64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestResult {
    pub name: String,
    pub seed: u64,
    /// None means passed. Failure text is identical across execution engines.
    pub failure: Option<String>,
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
) -> Result<Vec<Test>, ToolError> {
    let check = program.check().map_err(|e| program.error(e))?;
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
    let mut definitions = Vec::new();
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
        for id in &parsed.value.roots {
            let item = &parsed.value.tree.items[id.0 as usize];
            names.insert(item.definition.clone());
            if item
                .attributes
                .iter()
                .any(|a| parsed.value.symbols.resolve(a.name) == Some("test"))
                && (filters.is_empty()
                    || filters
                        .iter()
                        .any(|f| item.definition.to_string().contains(f)))
            {
                definitions.push(item.definition.clone());
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
        let unit = matches!(signature.types.get(f.result).map(|t| &t.kind), Some(TypeKind::Tuple(fields)) if fields.is_empty());
        let mut entry = definition.clone();
        let mut ordinal = 0;
        let mut context_name = definition.clone();
        loop {
            entry.0.name = format!("hirsel_test_entry_{ordinal}");
            context_name.0.name = format!("hirsel_test_context_{ordinal}");
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
        let source = if unit {
            format!(
                "@track_caller\npub fn {}({}: core@1/Test) -> core@1/Result[(), ()] {{\n  {call}\n  Ok(())\n}}\n",
                entry.0.name, context_name.0.name
            )
        } else {
            format!(
                "@track_caller\npub fn {}({}: core@1/Test) -> core@1/Result[(), ()] {{\n  match {call} {{ Ok(_) => Ok(()), Error(_) => Error(()) }}\n}}\n",
                entry.0.name, context_name.0.name
            )
        };
        let file = program
            .engine
            .sources
            .add(format!("<test entry {definition}>"), source)
            .ok_or(ToolError::SourceTooLarge)?;
        let context = FileContext {
            package: definition.0.package.clone(),
            modules: definition.0.modules.clone(),
        };
        let parsed = program
            .engine
            .parse_file(file, &context)
            .map_err(|e| program.error(e))?;
        if !parsed.diagnostics.is_empty() {
            return Err(program.error(hirsel_engine::QueryError::Diagnostics(parsed.diagnostics)));
        }
        program
            .engine
            .lower_to_tir(&entry)
            .map_err(|e| program.error(e))?;
        tests.push(Test {
            seed: override_seed.unwrap_or_else(|| seed(&definition)),
            definition,
            entry,
        });
    }
    Ok(tests)
}

pub fn run_interpreted(
    program: &mut LoadedProgram,
    tests: &[Test],
    checked: bool,
) -> Vec<TestResult> {
    tests
        .iter()
        .map(|test| {
            let failure = match execute(program, test, checked, 0) {
                Ok(execution) => match execution.outcome {
                    Ok(outcome) => outcome_failure(&outcome),
                    Err(error) => Some(format!("runtime error: {error:?}")),
                },
                Err(error) => Some(format!("runtime error: {error}")),
            };
            TestResult {
                name: test.definition.to_string(),
                seed: test.seed,
                failure,
            }
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
        ..Default::default()
    };
    let run = if checked {
        hirsel_engine::run_observed_checked
    } else {
        hirsel_engine::run_observed_snapshot
    };
    run(&mut program.engine, &config, simulation)
}

pub(crate) fn outcome_failure(outcome: &RunOutcome) -> Option<String> {
    match outcome {
        RunOutcome::Exit(0) => None,
        RunOutcome::Exit(1) => Some("returned Error".into()),
        RunOutcome::Blocked(_) => Some("runtime failure: blocked".into()),
        RunOutcome::Failure {
            rule,
            site,
            operation,
        } => Some(site.report(rule.as_str(), false, operation.as_ref())),
        outcome => Some(format!("runtime failure: {outcome:?}")),
    }
}

pub fn run_path(
    path: &Path,
    filters: &[String],
    override_seed: Option<u64>,
) -> Result<Vec<TestResult>, ToolError> {
    let mut program = crate::COMPILER.load_test_package(path)?;
    let tests = discover(&mut program, filters, override_seed)?;
    Ok(run_interpreted(&mut program, &tests, false))
}
