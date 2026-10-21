//! A complete compiler: the query engine over one set of front phases. Path
//! entry points start fresh sessions on the compiler thread, so a compiler is
//! the plain constructor of its phases and crosses threads freely.
use crate::*;
use std::{io::Write, path::Path, sync::Arc};

#[derive(Clone, Copy)]
pub struct Compiler {
    phases: fn() -> Arc<dyn Phases>,
    /// Digest of the compiler's own sources; std images record the one they
    /// were checked under (docs/building.md, "The std image").
    identity: [u8; 32],
    /// Whether the sessions this compiler starts run V.
    verify: Verify,
    comptime_limits: Option<hirsel_interp::Limits>,
}
impl Compiler {
    /// A compiler whose sessions run V.
    pub const fn new(phases: fn() -> Arc<dyn Phases>, identity: [u8; 32]) -> Self {
        Self {
            phases,
            identity,
            verify: Verify::On,
            comptime_limits: None,
        }
    }
    /// This compiler with V `verify` (`--no-verify` selects `Verify::Off`).
    pub const fn with_verify(self, verify: Verify) -> Self {
        Self { verify, ..self }
    }
    pub fn verify(&self) -> Verify {
        self.verify
    }
    /// Compile-time limits are part of every image's checked inputs.
    pub const fn with_comptime_limits(self, limits: hirsel_interp::Limits) -> Self {
        Self {
            comptime_limits: Some(limits),
            ..self
        }
    }
    pub(crate) fn comptime_limits(&self) -> hirsel_interp::Limits {
        self.comptime_limits.unwrap_or_default()
    }
    /// A fresh session engine of this compiler.
    pub(crate) fn engine(&self, environment: QueryEnvironment) -> QueryEngine {
        let mut engine = QueryEngine::new(environment, self.adapters());
        engine.verify = self.verify;
        engine.comptime_limits = self.comptime_limits();
        engine.session = true;
        engine.store_identity = self.identity();
        if crate::policy::uses_persistent_results() {
            let name = std::env::var("HIRSEL_RESULTS").unwrap_or_else(|_| "dir@1".into());
            if std::env::var_os("HIRSEL_VERIFY_CACHE").is_some_and(|v| v != "0") {
                engine.verify_store = crate::store::VerifyStore::Recompute;
            }
            engine.result_store = crate::store::store_by_name(
                &name,
                std::env::var_os("HIRSEL_RESULTS_DIR").map(Into::into),
            )
            .ok();
        }
        engine
    }
    pub(crate) fn identity(&self) -> ContentHash {
        ContentHash(self.identity)
    }
    /// A fresh complete adapter set with immutable shared front phases.
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
    /// or any body checking, compile-time evaluation or image loading.
    pub fn load_package_declarations(
        &self,
        manifest: &Path,
    ) -> Result<PackageDeclarations, ToolError> {
        crate::package::load_package_declarations(self, manifest)
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
        self.load_program_source_with_images(path, replacement, &StdImages::toolchain())
    }
    /// Load adopting exactly `images` (`StdImages::none()` checks every
    /// package from source).
    pub fn load_program_source_with_images(
        &self,
        path: &Path,
        replacement: Option<&str>,
        images: &StdImages,
    ) -> Result<LoadedProgram, ToolError> {
        crate::package::load_program_source_with_images(self, path, replacement, images, false)
    }
    /// Load an explicit test package without requiring a launch entry. The root
    /// package's sources are checked in full; no function is selected as main.
    pub fn load_test_package(&self, manifest: &Path) -> Result<LoadedProgram, ToolError> {
        if manifest.extension().is_none_or(|e| e != "json") {
            return Err(ToolError::Runtime(
                "test requires a package manifest JSON path".into(),
            ));
        }
        crate::package::load_program_source_with_images(
            self,
            manifest,
            None,
            &StdImages::toolchain(),
            true,
        )
    }
    /// Write to `output` the std image of the package `manifest` describes,
    /// checked freshly as the only dependency of an empty program
    /// (docs/building.md, "The std image"). Readers never see a partial image.
    pub fn image_package(&self, manifest: &Path, output: &Path) -> Result<(), ToolError> {
        // Programs trust an image's TIR as validated.
        if self.verify == Verify::Off {
            return Err(ToolError::Runtime("an image always runs V".into()));
        }
        let manifest = std::fs::canonicalize(manifest)?;
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest)?)
            .map_err(|e| ToolError::Runtime(format!("image: {}: {e}", manifest.display())))?;
        let package = &value["package"];
        let (Some(name), Some(version)) = (package["name"].as_str(), package["version"].as_str())
        else {
            return Err(ToolError::Runtime(format!(
                "image: {} names no package",
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
        let dir = std::env::temp_dir().join(format!("hirsel-image-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let program = dir.join("image.hir");
        let result = (|| {
            std::fs::write(&program, "pub fn main() -> Result[(), ()] {\n  Ok(())\n}\n")?;
            let sidecar = serde_json::json!({
                "schema": 1,
                "package": {"name": "hirsel_image", "version": "1"},
                "sources": [{"path": "image.hir", "module": []}],
                "dependencies": [{
                    "manifest": manifest,
                    "package": {"name": name, "version": version},
                    "expose": [],
                }],
                "entry": {"module": [], "name": "main"},
            });
            std::fs::write(dir.join("image.hir.json"), sidecar.to_string())?;
            // Records spill to a scratch file beside the program, so the
            // checked package is never held in memory.
            let spill = std::fs::File::options()
                .read(true)
                .write(true)
                .create_new(true)
                .open(dir.join("image.records"))?;
            let bytes = with_compiler_stack(|| {
                let mut loaded =
                    self.load_program_source_with_images(&program, None, &StdImages::none())?;
                loaded.image(&package, spill)
            })?;
            hirsel_image::write_atomically(output, &bytes)?;
            Ok(())
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
