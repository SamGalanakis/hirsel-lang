//! Compile one package test executable and launch each test in a fresh process.
use super::{Backend, NativeError, OptLevel, Options, Partition, StageName, TargetSpec, Toolchain};
use crate::testing::{Test, TestResult};
use hirsel_engine::{LoadedProgram, ToolError};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub fn test_path(
    path: &Path,
    filters: &[String],
    seed: Option<u64>,
    backend: Backend,
) -> Result<Vec<TestResult>, ToolError> {
    let mut loaded = crate::COMPILER.load_test_package(path)?;
    let tests = crate::testing::discover(&mut loaded, filters, seed)?;
    if tests.is_empty() {
        return Ok(vec![]);
    }
    run(&mut loaded, &tests, backend).map_err(|e| ToolError::Runtime(e.to_string()))
}

fn run(
    loaded: &mut LoadedProgram,
    tests: &[Test],
    backend: Backend,
) -> Result<Vec<TestResult>, NativeError> {
    let input = loaded
        .engine
        .export_native_input()
        .map_err(|e| NativeError::new(StageName::Manifest, format!("{e:?}")))?;
    let sites = super::Sites::new(&input);
    let validated = loaded
        .engine
        .import_native_input(input.clone())
        .map_err(|e| NativeError::new(StageName::Manifest, format!("{e:?}")))?;
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut program = super::build(&validated, &Options::new(target.clone()))?;
    let scratch = super::executable::Scratch::new()?;
    let cache = std::env::var_os("HIRSEL_NATIVE_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(super::runtime_cache::default_directory);
    let tools = Toolchain::in_tree(&cache, &[&target])?;
    let objects = super::emit_backend_objects(
        &program,
        backend,
        OptLevel::None,
        Partition::Whole,
        false,
        None,
    )?;
    let executable = super::link::link_tests(&mut program, tests, &objects, &tools, &scratch.0)?;
    let record = scratch.0.join("observation");
    let mut results = Vec::new();
    for (index, test) in tests.iter().enumerate() {
        let _ = std::fs::remove_file(&record);
        let output = Command::new(&executable)
            .arg(index.to_string())
            .env_clear()
            .env("HIRSEL_RT_OBSERVATION", &record)
            .output()
            .map_err(|e| NativeError::new(StageName::Launch, e.to_string()))?;
        let launched = super::Launched {
            audit_required: false,
            exit: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
            record: std::fs::read_to_string(&record).unwrap_or_default(),
        };
        let failure = match super::observe(&launched, &program, &sites) {
            Ok(observation) if observation.exit_code == Some(0) => None,
            Ok(observation) if observation.exit_code == Some(1) => Some("returned Error".into()),
            Ok(observation) => observation
                .failures
                .first()
                .map(|f| format!("runtime failure[{}] at line {}", f.rule, f.line))
                .or_else(|| Some("runtime failure: blocked".into())),
            Err(error) => Some(format!("runtime error: {error}")),
        };
        results.push(TestResult {
            name: test.definition.to_string(),
            seed: test.seed,
            failure,
        });
    }
    Ok(results)
}
