// Tests select the same ordinary package graph as applications.
use hirsel_engine::{
    CheckCompletion, CheckOutput, CompileConfig, Compiler, ComptimeLimits, LoadedProgram,
    LoweringPolicy, QueryEngine, ToolError,
    loader_inputs::{CaptureInputs, FrozenInputs, InputLimits},
    store::MemStore,
};
use std::{
    path::{Path, PathBuf},
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
    let (compiler, inputs, captured) = compiler_for(path, lowering, limits)?;
    let program = compiler.load_program_inputs(path, &inputs, true).unwrap();
    Ok((program, captured))
}

fn compiler_for(
    path: &Path,
    lowering: LoweringPolicy,
    limits: ComptimeLimits,
) -> Result<(Compiler, FrozenInputs, FrozenInputs), CheckOutput> {
    let capture = CaptureInputs::new(InputLimits {
        bytes: 64 << 20,
        operations: 100_000,
    });
    let (unsafe_ffi, primary) = {
        let program = hirsel_driver::COMPILER
            .load_program_inputs(path, &capture, false)
            .unwrap();
        (
            program.unsafe_ffi.clone(),
            program
                .engine
                .sources
                .file(program.primary)
                .unwrap()
                .path
                .clone(),
        )
    };
    let captured = capture.freeze();
    let inputs = captured
        .replace_sources(&[(primary, Arc::<[u8]>::from(&b""[..]))].into(), 64 << 20)
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
        let result = requests.execute_request(&CompileRequest {
            path: path.to_owned(),
            inputs: key.inputs.clone(),
            operation: CompileOperation::Check { lowering },
        });
        let output = match result {
            Ok(completed) => {
                let CompileResult::Check(output) = &completed.result else {
                    panic!("expected fixture Check")
                };
                output.clone()
            }
            Err(ToolError::Query {
                error: hirsel_types::QueryError::Diagnostics(diagnostics),
                sources,
                ..
            }) => CheckOutput {
                diagnostics: hirsel_engine::select_diagnostics(diagnostics, Some(&sources)),
                sources,
                unsafe_ffi,
                completion: CheckCompletion::Complete,
            },
            Err(error) => panic!("fixture Check: {error}"),
        };
        if output.completion != CheckCompletion::Complete || !output.diagnostics.is_empty() {
            return Err(output);
        }
        checked.push(Checked { key, store });
    }
    drop(checked);
    Ok((compiler, inputs, captured))
}

/// A source fixture owns one immutable input capture and one admitted selector.
/// Each overlay is a new request. Carry and source-index reuse remain inside the
/// compiler; returned TIR retains its own old snapshot across later overlays.
#[allow(dead_code)]
pub struct TirFixture {
    path: PathBuf,
    primary: PathBuf,
    inputs: FrozenInputs,
    entry: hirsel_syntax::DefPath,
    requests: hirsel_driver::requests::CompilerRequests,
}
#[allow(dead_code)]
impl TirFixture {
    pub fn new(path: &Path) -> Result<Self, ToolError> {
        hirsel_engine::with_compiler_stack(|| {
            let (compiler, _, inputs) =
                compiler_for(path, LoweringPolicy::Reachable, ComptimeLimits::default()).map_err(
                    |output| ToolError::Query {
                        error: hirsel_types::QueryError::Diagnostics(output.diagnostics),
                        phase: None,
                        sources: output.sources,
                    },
                )?;
            let program = compiler.load_program_inputs(path, &inputs, false)?;
            let entry = program.entry.clone();
            let primary = program
                .engine
                .sources
                .file(program.primary)
                .ok_or(ToolError::SourceTooLarge)?
                .path
                .clone();
            drop(program);
            Ok(Self {
                path: path.to_owned(),
                primary,
                inputs,
                entry,
                requests: hirsel_driver::requests::CompilerRequests::for_host(
                    compiler.clone(),
                    compiler.configuration(),
                ),
            })
        })
    }
    pub fn tir(
        &mut self,
        replacement: Option<&str>,
        name: Option<&str>,
    ) -> Result<Arc<hirsel_driver::requests::TirExecution>, ToolError> {
        use hirsel_driver::requests::*;
        let mut entry = self.entry.clone();
        if let Some(name) = name {
            entry.0.name = name.into();
        }
        let output = self.select(
            replacement,
            CompileOperation::Tir {
                definition: Some(entry),
            },
        )?;
        let CompileResult::Tir(tir) = &output.result else {
            return Err(ToolError::Runtime(
                "source fixture returned another operation".into(),
            ));
        };
        Ok(tir.clone())
    }
    pub fn batch(
        &mut self,
        source: &str,
        names: &[String],
    ) -> Result<Vec<Arc<hirsel_driver::requests::TirExecution>>, ToolError> {
        use hirsel_driver::requests::*;
        let definitions = names
            .iter()
            .map(|name| {
                let mut entry = self.entry.clone();
                entry.0.name = name.into();
                entry
            })
            .collect::<Vec<_>>();
        let output = self.select(
            Some(source),
            CompileOperation::TirBatch {
                definitions: definitions.clone(),
            },
        )?;
        let CompileResult::TirBatch(batch) = &output.result else {
            return Err(ToolError::Runtime(
                "source fixture returned another operation".into(),
            ));
        };
        definitions.iter().map(|entry| batch.entry(entry)).collect()
    }
    fn select(
        &mut self,
        replacement: Option<&str>,
        operation: hirsel_driver::requests::CompileOperation,
    ) -> Result<Arc<hirsel_driver::requests::CompileOutput>, ToolError> {
        use hirsel_driver::requests::*;
        let inputs = if let Some(source) = replacement {
            self.inputs.replace_sources(
                &[(self.primary.clone(), Arc::<[u8]>::from(source.as_bytes()))].into(),
                self.requests.input_limits().bytes,
            )?
        } else {
            self.inputs.clone()
        };
        self.requests.execute_request(&CompileRequest {
            path: self.path.clone(),
            inputs,
            operation,
        })
    }
}

/// Checked immutable TIR for one exact source overlay and selected function.
#[allow(dead_code)]
pub fn tir(
    path: &Path,
    replacement: Option<&str>,
    name: Option<&str>,
) -> Result<Arc<hirsel_driver::requests::TirExecution>, ToolError> {
    TirFixture::new(path)?.tir(replacement, name)
}
