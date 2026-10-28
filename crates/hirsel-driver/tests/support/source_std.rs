// Tests select the same ordinary package graph as applications.
use hirsel_engine::{
    CheckCompletion, CheckOutput, CompileConfig, ComptimeLimits, LoadedProgram, LoweringPolicy,
    QueryEngine, ToolError,
    loader_inputs::{CaptureInputs, FrozenInputs, InputLimits},
    store::MemStore,
};
use std::{
    path::Path,
    sync::{Arc, Mutex, OnceLock},
};

#[derive(PartialEq)]
struct Key {
    inputs: FrozenInputs,
    lowering: LoweringPolicy,
    limits: ComptimeLimits,
    jobs: usize,
    hold_bytes: u64,
}
struct Checked {
    key: Key,
    store: Arc<MemStore>,
}

pub fn engine() -> QueryEngine {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/hirsel-driver/tests/fixtures/std_source/main.hir");
    engine_for(&path, LoweringPolicy::Reachable, ComptimeLimits::default()).unwrap_or_else(
        |output| {
            panic!(
                "std setup: {:?}: {:?}",
                output.completion, output.diagnostics
            )
        },
    )
}

// Capture the complete selected graph on each request, including source bytes
// and failed path probes. A source or manifest edit therefore selects a fresh
// check. Compiler identity is fixed by this test executable.
pub fn engine_for(
    path: &Path,
    lowering: LoweringPolicy,
    limits: ComptimeLimits,
) -> Result<QueryEngine, CheckOutput> {
    program_for(path, lowering, limits).map(|(program, _)| program.engine)
}

#[allow(dead_code)] // The other integration binary uses only engine fixtures.
pub fn load_program_source(
    path: &Path,
    replacement: Option<&str>,
) -> Result<LoadedProgram, ToolError> {
    let (mut program, captured) =
        program_for(path, LoweringPolicy::Reachable, ComptimeLimits::default()).map_err(
            |output| ToolError::Query {
                error: hirsel_types::QueryError::Diagnostics(output.diagnostics),
                phase: None,
                sources: output.sources,
            },
        )?;
    let primary = std::fs::canonicalize(path)?;
    let source = match replacement {
        Some(source) => source.to_owned(),
        None => String::from_utf8(
            hirsel_engine::loader_inputs::LoaderInputs::read(&captured, &primary)?.to_vec(),
        )
        .map_err(|error| ToolError::Runtime(error.to_string()))?,
    };
    assert!(program.replace_source(&primary, source)?);
    program.parse_sources()?;
    Ok(program)
}

fn program_for(
    path: &Path,
    lowering: LoweringPolicy,
    limits: ComptimeLimits,
) -> Result<(LoadedProgram, FrozenInputs), CheckOutput> {
    let capture = CaptureInputs::new(InputLimits {
        bytes: 64 << 20,
        operations: 100_000,
    });
    hirsel_driver::COMPILER
        .load_program_inputs(path, &capture, false)
        .unwrap();
    let captured = capture.freeze();
    let inputs = captured
        .replace_sources(
            &[(
                std::fs::canonicalize(path).unwrap(),
                Arc::<[u8]>::from(&b""[..]),
            )]
            .into(),
            64 << 20,
        )
        .unwrap();
    let config = CompileConfig {
        lowering,
        ..CompileConfig::default()
    };
    let key = Key {
        inputs,
        lowering,
        limits,
        jobs: config.jobs.get(),
        hold_bytes: config.hold_bytes,
    };
    static CHECKED: OnceLock<Mutex<Vec<Checked>>> = OnceLock::new();
    let mut checked = CHECKED.get_or_init(Default::default).lock().unwrap();
    let existing = checked.iter().find(|entry| entry.key == key);
    let store = existing.map_or_else(
        || Arc::new(MemStore::default()),
        |entry| entry.store.clone(),
    );
    let compiler = hirsel_driver::COMPILER
        .with_owned_engines()
        .with_config(CompileConfig {
            store: store.clone(),
            ..config
        })
        .with_comptime_limits(limits);
    let inputs = key.inputs.clone();
    if existing.is_none() {
        use hirsel_driver::requests::*;
        let mut requests = CompilerRequests::for_host(compiler.clone(), compiler.configuration());
        let output = requests
            .execute_request(&CompileRequest {
                path: path.to_owned(),
                inputs: key.inputs.clone(),
                operation: CompileOperation::Check { lowering },
            })
            .unwrap();
        let CompileResult::Check(output) = &output.result else {
            panic!("expected fixture Check")
        };
        let output = output.clone();
        if output.completion != CheckCompletion::Complete || !output.diagnostics.is_empty() {
            return Err(output);
        }
        checked.push(Checked { key, store });
    }
    drop(checked);
    let program = compiler.load_program_inputs(path, &inputs, true).unwrap();
    // Each caller owns a fresh source map, memo graph and coordinator. Only
    // content-addressed immutable products cross requests, through the ordinary
    // engine store. Later lowerings publish there for subsequent callers too.
    Ok((program, captured))
}

/// Checked immutable TIR for one exact source overlay and selected function.
#[allow(dead_code)]
pub fn tir(
    path: &Path,
    replacement: Option<&str>,
    name: Option<&str>,
) -> Result<Arc<hirsel_driver::requests::TirExecution>, ToolError> {
    use hirsel_driver::requests::*;
    let config = hirsel_driver::COMPILER.configuration();
    let compiler = hirsel_driver::COMPILER
        .with_owned_engines()
        .with_config(config.clone());
    let mut requests = CompilerRequests::for_host(compiler.clone(), config);
    let (program, inputs) = hirsel_engine::requests::observe_inputs(
        requests.input_limits(),
        requests.capture_attempts(),
        |capture| compiler.load_program_inputs(path, capture, false),
    )
    .map_err(|_| ToolError::Runtime("fixture inputs kept changing during discovery".into()))?;
    let program = program?;
    let mut entry = program.entry.clone();
    if let Some(name) = name {
        entry.0.name = name.into();
    }
    drop(program);
    let inputs = if let Some(source) = replacement {
        inputs.replace_sources(
            &[(
                std::fs::canonicalize(path)?,
                Arc::<[u8]>::from(source.as_bytes()),
            )]
            .into(),
            requests.input_limits().bytes,
        )?
    } else {
        inputs
    };
    let output = requests.execute_request(&CompileRequest {
        path: path.to_owned(),
        inputs,
        operation: CompileOperation::Tir {
            definition: Some(entry),
        },
    })?;
    let CompileResult::Tir(tir) = &output.result else {
        return Err(ToolError::Runtime(
            "source fixture returned another operation".into(),
        ));
    };
    Ok(tir.clone())
}
