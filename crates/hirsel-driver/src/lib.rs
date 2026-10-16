//! The compiler: hirsel-engine's queries wired to the front phases it may not
//! depend on (signature, check, modes, lower) and to the native backends that
//! compile its validated native input. Callers import engine items from
//! hirsel-engine itself; this crate exports only the wiring.
use hirsel_engine::{
    CheckOutput, Compiler, LoadedProgram, NotImplemented, PackageCache, PhaseAdapters, RunOutcome,
    SimulationConfig, ToolError,
};
pub mod native;
mod pipeline;
pub use pipeline::{BodyAdapter, FrontPhases, LowerAdapter, ModeAdapter};
use std::{io::Write, path::Path};

/// The complete compiler over the front phases in [`FrontPhases`].
pub const COMPILER: Compiler = Compiler::new(FrontPhases::shared);

pub fn compiler_adapters() -> PhaseAdapters {
    COMPILER.adapters()
}
/// `.json` selects a launch manifest; `.hir` selects its exact `.hir.json`
/// sidecar if present, otherwise the explicit standalone@0 context.
pub fn load_program(path: &Path) -> Result<LoadedProgram, ToolError> {
    COMPILER.load_program(path)
}
/// Corpus transformations replace only the primary source, retaining its package graph.
pub fn load_program_source(
    path: &Path,
    replacement: Option<&str>,
) -> Result<LoadedProgram, ToolError> {
    COMPILER.load_program_source(path, replacement)
}
/// Load with explicit artifact storage, or `PackageCache::disabled()` for a fresh oracle.
pub fn load_program_source_with_cache(
    path: &Path,
    replacement: Option<&str>,
    cache: &PackageCache,
) -> Result<LoadedProgram, ToolError> {
    COMPILER.load_program_source_with_cache(path, replacement, cache)
}
/// A partial pipeline can produce rejection diagnostics, but never a clean acceptance.
pub fn check_path(path: &Path) -> Result<CheckOutput, ToolError> {
    COMPILER.check_path(path)
}
pub fn format_path(path: &Path) -> Result<String, ToolError> {
    COMPILER.format_path(path)
}
pub fn run_path(path: &Path, stdout: &mut dyn Write) -> Result<RunOutcome, ToolError> {
    COMPILER.run_path(path, stdout)
}
pub fn run_path_with_simulation(
    path: &Path,
    simulation: SimulationConfig,
    stdout: &mut dyn Write,
) -> Result<RunOutcome, ToolError> {
    COMPILER.run_path_with_simulation(path, simulation, stdout)
}
/// `hirsel export --tir <source|manifest.json>`: the proof-seam TIR export
///  of the checked program's entry closure.
pub fn export_tir_path(path: &Path) -> Result<String, ToolError> {
    hirsel_engine::with_compiler_stack(|| {
        let mut program = load_program(path)?;
        program.check_for_execution()?;
        let entry = program.entry.clone();
        program
            .engine
            .export_tir(&entry)
            .map_err(|error| program.error(error))
    })
}
/// Narrow conformance adapter returns diagnostics with revision-1 completion.
pub fn check_file(path: &Path) -> Result<CheckOutput, NotImplemented> {
    check_path(path).map_err(|_| NotImplemented)
}
pub fn run_file(path: &Path, stdout: &mut dyn Write) -> Result<RunOutcome, NotImplemented> {
    run_path(path, stdout).map_err(|_| NotImplemented)
}
