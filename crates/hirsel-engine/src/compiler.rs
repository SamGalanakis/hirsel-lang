//! A complete compiler: the query engine over one set of front phases. Path
//! entry points start fresh sessions on the compiler thread, so a compiler is
//! the plain constructor of its phases and crosses threads freely.
use crate::*;
use std::{io::Write, path::Path, rc::Rc};

#[derive(Clone, Copy)]
pub struct Compiler {
    phases: fn() -> Rc<dyn Phases>,
    /// Digest of the compiler's own sources; prechecks record the one they were
    /// checked under (docs/building.md, "The std precheck").
    identity: [u8; 32],
}
impl Compiler {
    pub const fn new(phases: fn() -> Rc<dyn Phases>, identity: [u8; 32]) -> Self {
        Self { phases, identity }
    }
    pub(crate) fn identity(&self) -> ContentHash {
        ContentHash(self.identity)
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
    /// The precheck of the package `manifest` describes, checked freshly as the only
    /// dependency of an empty program (docs/building.md, "The std precheck").
    pub fn precheck_package(&self, manifest: &Path) -> Result<Vec<u8>, ToolError> {
        let manifest = std::fs::canonicalize(manifest)?;
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest)?)
            .map_err(|e| ToolError::Runtime(format!("precheck: {}: {e}", manifest.display())))?;
        let package = &value["package"];
        let (Some(name), Some(version)) = (package["name"].as_str(), package["version"].as_str())
        else {
            return Err(ToolError::Runtime(format!(
                "precheck: {} names no package",
                manifest.display()
            )));
        };
        let package = hirsel_syntax::PackageId {
            name: name.into(),
            version: version.into(),
        };
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir =
            std::env::temp_dir().join(format!("hirsel-precheck-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let program = dir.join("precheck.hir");
        let result = (|| {
            std::fs::write(&program, "pub fn main() -> Result[(), ()] {\n  Ok(())\n}\n")?;
            let sidecar = serde_json::json!({
                "schema": 1,
                "package": {"name": "hirsel_precheck", "version": "1"},
                "sources": [{"path": "precheck.hir", "module": []}],
                "dependencies": [{
                    "manifest": manifest,
                    "package": {"name": name, "version": version},
                    "expose": [],
                }],
                "entry": {"module": [], "name": "main"},
            });
            std::fs::write(dir.join("precheck.hir.json"), sidecar.to_string())?;
            with_compiler_stack(|| {
                let mut loaded =
                    self.load_program_source_with_cache(&program, None, &PackageCache::disabled())?;
                loaded.precheck(&package)
            })
        })();
        let _ = std::fs::remove_dir_all(&dir);
        result
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
