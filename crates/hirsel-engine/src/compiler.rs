//! A complete compiler: the query engine over one set of front phases. Path
//! entry points start fresh sessions on the compiler thread, so a compiler is
//! the plain constructor of its phases and crosses threads freely.
use crate::*;
use std::{io::Write, path::Path, rc::Rc};

#[derive(Clone, Copy)]
pub struct Compiler {
    phases: fn() -> Rc<dyn Phases>,
}
impl Compiler {
    pub const fn new(phases: fn() -> Rc<dyn Phases>) -> Self {
        Self { phases }
    }
    /// A fresh complete adapter set with its own compiler state.
    pub fn adapters(&self) -> PhaseAdapters {
        PhaseAdapters::compiler((self.phases)())
    }
    pub(crate) fn load(&self, path: &Path) -> Result<(QueryEngine, FileId), ToolError> {
        let text = crate::tools::read_source(path)?;
        let mut engine = QueryEngine::new(default_environment(), self.adapters());
        let file = engine
            .sources
            .add(path, text)
            .ok_or(ToolError::SourceTooLarge)?;
        Ok((engine, file))
    }
    /// `.json` selects a launch manifest; `.hir` selects its exact `.hir.json`
    /// sidecar if present, otherwise the explicit standalone@0 context.
    pub fn load_program(&self, path: &Path) -> Result<LoadedProgram, ToolError> {
        self.load_program_source(path, None)
    }
    /// Corpus transformations replace only the primary source, retaining its package graph.
    pub fn load_program_source(
        &self,
        path: &Path,
        replacement: Option<&str>,
    ) -> Result<LoadedProgram, ToolError> {
        self.load_program_source_with_cache(path, replacement, &PackageCache::shared())
    }
    /// Load with explicit artifact storage, or `PackageCache::disabled()` for a fresh oracle.
    pub fn load_program_source_with_cache(
        &self,
        path: &Path,
        replacement: Option<&str>,
        cache: &PackageCache,
    ) -> Result<LoadedProgram, ToolError> {
        crate::package::load_program_source_with_cache(self, path, replacement, cache)
    }
    /// A partial pipeline can produce rejection diagnostics, but never a clean acceptance.
    pub fn check_path(&self, path: &Path) -> Result<CheckOutput, ToolError> {
        with_compiler_stack(|| {
            let mut program = self.load_program(path)?;
            program.check().map_err(|error| program.error(error))
        })
    }
    pub fn format_path(&self, path: &Path) -> Result<String, ToolError> {
        with_compiler_stack(|| {
            let (mut engine, file) = self.load(path)?;
            let result = format_snapshot(&mut engine, file, &default_context());
            result.map_err(|error| ToolError::Query {
                error,
                phase: engine.unavailable,
                sources: engine.sources.clone(),
            })
        })
    }
    pub fn run_path(&self, path: &Path, stdout: &mut dyn Write) -> Result<RunOutcome, ToolError> {
        self.run_path_with_simulation(path, SimulationConfig::default(), stdout)
    }
    pub fn run_path_with_simulation(
        &self,
        path: &Path,
        simulation: SimulationConfig,
        stdout: &mut dyn Write,
    ) -> Result<RunOutcome, ToolError> {
        let mut program = self.load_program(path)?;
        program.check_for_execution()?;
        program.refuse_foreign_interpretation()?;
        let config = RunConfiguration {
            entry: program.entry,
            hash_seed: 0,
            inputs: vec![],
        };
        run_snapshot(&mut program.engine, &config, simulation, stdout)
    }
}
