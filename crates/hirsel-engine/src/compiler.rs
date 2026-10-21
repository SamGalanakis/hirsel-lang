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
    /// Whether the sessions this compiler starts run V.
    verify: Verify,
}
impl Compiler {
    /// A compiler whose sessions run V.
    pub const fn new(phases: fn() -> Rc<dyn Phases>, identity: [u8; 32]) -> Self {
        Self {
            phases,
            identity,
            verify: Verify::On,
        }
    }
    /// This compiler with V `verify` (`--no-verify` selects `Verify::Off`).
    pub const fn with_verify(self, verify: Verify) -> Self {
        Self { verify, ..self }
    }
    pub fn verify(&self) -> Verify {
        self.verify
    }
    /// A fresh session engine of this compiler.
    pub(crate) fn engine(&self, environment: QueryEnvironment) -> QueryEngine {
        let mut engine = QueryEngine::new(environment, self.adapters());
        engine.verify = self.verify;
        engine.session = true;
        engine
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
        let mut engine = self.engine(default_environment());
        let file = engine
            .sources
            .add(path, text)
            .ok_or(ToolError::SourceTooLarge)?;
        Ok((engine, file))
    }
    /// Index an explicit package and its dependency declarations without an entry
    /// or any body checking, compile-time evaluation or precheck loading.
    pub fn load_package_declarations(
        &self,
        manifest: &Path,
    ) -> Result<PackageDeclarations, ToolError> {
        crate::package::load_package_declarations(self, manifest)
    }
    /// `.json` selects a launch manifest; `.hir` selects its exact `.hir.json`
    /// sidecar if present, otherwise standalone@0 with the toolchain std@1.
    pub fn load_program(&self, path: &Path) -> Result<LoadedProgram, ToolError> {
        self.load_program_source(path, None)
    }
    /// Corpus transformations replace only the primary source, retaining its package graph.
    pub fn load_program_source(
        &self,
        path: &Path,
        replacement: Option<&str>,
    ) -> Result<LoadedProgram, ToolError> {
        self.load_program_source_with_cache(path, replacement, &PackageCache::shared(self.verify))
    }
    /// Load with explicit artifact storage, or `PackageCache::disabled()` for a fresh oracle.
    /// Sessions with and without V never share `cache`.
    pub fn load_program_source_with_cache(
        &self,
        path: &Path,
        replacement: Option<&str>,
        cache: &PackageCache,
    ) -> Result<LoadedProgram, ToolError> {
        crate::package::load_program_source_with_cache(self, path, replacement, cache, false)
    }
    /// Load an explicit test package without requiring a launch entry. The root
    /// package's sources are checked in full; no function is selected as main.
    pub fn load_test_package(&self, manifest: &Path) -> Result<LoadedProgram, ToolError> {
        crate::package::load_program_source_with_cache(
            self,
            manifest,
            None,
            &PackageCache::shared(self.verify),
            true,
        )
    }
    /// Write to `output` the precheck of the package `manifest` describes, checked
    /// freshly as the only dependency of an empty program (docs/building.md, "The
    /// std precheck"). Readers never see a partial precheck.
    pub fn precheck_package(&self, manifest: &Path, output: &Path) -> Result<(), ToolError> {
        // Programs trust a precheck's TIR as validated.
        if self.verify == Verify::Off {
            return Err(ToolError::Runtime("precheck always runs V".into()));
        }
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
            // Records stream through a scratch file beside the program, so the
            // encoded package is never held in memory.
            let mut spill = std::fs::File::options()
                .read(true)
                .write(true)
                .create_new(true)
                .open(dir.join("precheck.body"))?;
            let partial = output.with_extension("precheck.partial");
            let written = (|| -> Result<(), ToolError> {
                let mut out = std::io::BufWriter::new(std::fs::File::create(&partial)?);
                with_compiler_stack(|| {
                    let mut loaded = self.load_program_source_with_cache(
                        &program,
                        None,
                        &PackageCache::disabled(),
                    )?;
                    loaded.precheck(&package, &mut out, &mut spill)
                })?;
                out.into_inner().map_err(|e| e.into_error())?;
                std::fs::rename(&partial, output)?;
                Ok(())
            })();
            if written.is_err() {
                let _ = std::fs::remove_file(&partial);
            }
            written
        })();
        let _ = std::fs::remove_dir_all(&dir);
        result
    }
    /// A partial pipeline can produce rejection diagnostics, but never a clean acceptance.
    pub fn check_path(&self, path: &Path) -> Result<CheckOutput, ToolError> {
        with_compiler_stack(|| {
            let mut program = self.load_program(path)?;
            program
                .check(LoweringPolicy::Comptime)
                .map_err(|error| program.error(error))
        })
        .or_else(check_diagnostics)
    }
    /// `check_path`, then, when the program checks complete and accepted, the
    /// escape from capability checking its entry reaches (CAP-6).
    pub fn check_safe_path(
        &self,
        path: &Path,
    ) -> Result<(CheckOutput, Option<(DefPath, Option<UnsafeReach>)>), ToolError> {
        with_compiler_stack(|| {
            let mut program = self.load_program(path)?;
            let output = program
                .check(LoweringPolicy::Comptime)
                .map_err(|error| program.error(error))?;
            if crate::queries::has_errors(&output.diagnostics)
                || output.completion != CheckCompletion::Complete
            {
                return Ok((output, None));
            }
            let reach = program.unsafe_reach()?;
            Ok((output, Some((program.entry.clone(), reach))))
        })
        .or_else(|error| check_diagnostics(error).map(|output| (output, None)))
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

/// Parsing may reject while the graph is indexed, before LoadedProgram exists.
/// Checks return those language diagnostics through the same output as body errors.
fn check_diagnostics(error: ToolError) -> Result<CheckOutput, ToolError> {
    match error {
        ToolError::Query {
            error: QueryError::Diagnostics(diagnostics),
            sources,
            ..
        } => Ok(CheckOutput {
            diagnostics: select_diagnostics(diagnostics, Some(&sources)),
            sources,
            ..Default::default()
        }),
        error => Err(error),
    }
}
