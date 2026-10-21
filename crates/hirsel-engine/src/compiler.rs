//! A complete compiler: the query engine over one set of front phases. Path
//! entry points start fresh sessions on the compiler thread, so a compiler is
//! the plain constructor of its phases and crosses threads freely.
use crate::*;
use std::{io::Write, path::Path, sync::Arc};

#[derive(Clone)]
pub struct Compiler {
    owned_engines: bool,
    phases: fn() -> Arc<dyn Phases>,
    /// Digest of the compiler's own sources; std images record the one they
    /// were checked under (docs/building.md, "The std image").
    identity: [u8; 32],
    /// Whether the sessions this compiler starts run V.
    verify: Verify,
    retention: crate::memo::Retention,
    jobs: Option<hirsel_intern::jobs::Jobs>,
    hold_bytes: Option<u64>,
    lowering: LoweringPolicy,
    result_store: Option<Arc<dyn crate::store::ResultStore>>,
    verify_store: crate::store::VerifyStore,
    comptime_limits: Option<hirsel_interp::Limits>,
}
impl Compiler {
    /// A compiler whose sessions run V.
    pub const fn new(phases: fn() -> Arc<dyn Phases>, identity: [u8; 32]) -> Self {
        Self {
            owned_engines: false,
            phases,
            identity,
            verify: Verify::On,
            retention: crate::memo::Retention::Batch,
            jobs: None,
            hold_bytes: None,
            lowering: LoweringPolicy::Comptime,
            result_store: None,
            verify_store: crate::store::VerifyStore::Off,
            comptime_limits: None,
        }
    }
    /// This compiler with V `verify` (`--no-verify` selects `Verify::Off`).
    pub const fn with_verify(mut self, verify: Verify) -> Self {
        self.verify = verify;
        self
    }
    /// Retain intermediate front bodies only for an explicit resident host.
    pub const fn with_retention(mut self, retention: crate::memo::Retention) -> Self {
        self.retention = retention;
        self
    }
    /// Own explicit per-session policies and a shared result-store provider.
    /// Cloning a compiler shares that provider; sessions retain separate memos.
    pub fn with_config(mut self, config: CompileConfig) -> Self {
        self.verify = config.verify;
        self.retention = config.retention;
        self.jobs = Some(config.jobs);
        self.hold_bytes = Some(config.hold_bytes);
        self.lowering = config.lowering;
        self.verify_store = config.verify_store;
        self.result_store = Some(config.store);
        self
    }
    /// Engines opened by a library host always reclaim their private domains,
    /// even if a CLI has selected process-exit reclamation globally.
    pub fn with_owned_engines(mut self) -> Self {
        self.owned_engines = true;
        self
    }
    pub fn verify(&self) -> Verify {
        self.verify
    }
    /// Compile-time limits are part of every image's checked inputs.
    pub const fn with_comptime_limits(mut self, limits: hirsel_interp::Limits) -> Self {
        self.comptime_limits = Some(limits);
        self
    }
    pub(crate) fn comptime_limits(&self) -> hirsel_interp::Limits {
        self.comptime_limits.unwrap_or_default()
    }
    /// A fresh session engine of this compiler.
    pub(crate) fn engine(&self, environment: QueryEnvironment) -> QueryEngine {
        let mut engine = QueryEngine::new(environment, self.adapters());
        engine.verify = self.verify;
        engine.set_retention(self.retention);
        engine.configured_jobs = self.jobs;
        engine.configured_lowering = self.lowering;
        engine.set_hold_bytes(
            self.hold_bytes
                .unwrap_or_else(crate::config::configured_hold_bytes),
        );
        engine.verify_store = self.verify_store;
        engine.comptime_limits = self.comptime_limits();
        engine.session = !self.owned_engines;
        engine.store_identity = self.identity();
        if let Some(store) = &self.result_store {
            engine = engine.with_result_store(store.clone());
        } else if crate::policy::uses_persistent_results() {
            let name = std::env::var("HIRSEL_RESULTS").unwrap_or_else(|_| "dir@1".into());
            if crate::policy::verifies_cached_results()
                || std::env::var_os("HIRSEL_VERIFY_CACHE").is_some_and(|v| v != "0")
            {
                engine.verify_store = crate::store::VerifyStore::Recompute;
            }
            if let Ok(store) = crate::store::store_by_name(
                &name,
                std::env::var_os("HIRSEL_RESULTS_DIR")
                    .filter(|v| !v.is_empty())
                    .map(Into::into),
            ) {
                engine = engine.with_result_store(store);
            }
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
        self.load_inputs(path, &crate::loader_inputs::DiskInputs)
    }
    pub(crate) fn load_inputs(
        &self,
        path: &Path,
        inputs: &dyn crate::loader_inputs::LoaderInputs,
    ) -> Result<(QueryEngine, FileId), ToolError> {
        let text = crate::tools::decode_source(path, inputs.read(path)?.to_vec())?;
        let mut engine = self.engine(default_environment());
        let file = engine
            .sources
            .add(path, text)
            .ok_or(ToolError::SourceTooLarge)?;
        Ok((engine, file))
    }
    /// Index an explicit package and its dependency declarations without an entry
    /// or any body checking, compile-time evaluation or image loading. The
    /// returned engine retains this compiler's session policies for later queries.
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
    /// Load exclusively from explicit input observations and immutable image
    /// handles. Discovery closes the package/input set without source parsing,
    /// so a syntax-error base can still be repaired. Manifest errors remain
    /// loader errors. Unlike ordinary loading, this never rebuilds an image.
    pub fn load_program_inputs(
        &self,
        path: &Path,
        inputs: &dyn crate::loader_inputs::LoaderInputs,
        images: &StdImages,
        parse_sources: bool,
    ) -> Result<LoadedProgram, ToolError> {
        crate::package::load_program_inputs(
            self,
            path,
            inputs,
            crate::package::Loading {
                replacement: None,
                images,
                package_only: false,
                parse_sources,
                rebuild_images: false,
            },
        )
    }
    /// Load an explicit test package without requiring a launch entry. The root
    /// package's sources are checked in full; no function is selected as main.
    pub fn load_test_package(&self, manifest: &Path) -> Result<LoadedProgram, ToolError> {
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
                .check(self.lowering)
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
                .check(self.lowering)
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

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_span::Span;
    use hirsel_syntax::ItemSyntax;
    use hirsel_types::{Resolution, Signature, TypeInterner, TypeMemo};
    use std::sync::Arc;

    struct NoBodies;
    impl CheckerPhase for NoBodies {
        fn check(
            &self,
            _t: &hirsel_types::TypeInterner,
            _m: &hirsel_types::TypeMemo,
            _syntax: std::sync::Arc<ItemSyntax>,
            _resolution: Resolution,
            _signature: &Signature,
            _signatures: &mut dyn SignatureProvider,
        ) -> Result<QueryOutput<TypedBody>, QueryError> {
            panic!("declaration loading entered a body phase")
        }
    }
    impl ModesPhase for NoBodies {
        fn check_modes(
            &self,
            _t: &hirsel_types::TypeInterner,
            _m: &hirsel_types::TypeMemo,
            _body: std::sync::Arc<TypedBody>,
            _signature: &Signature,
            _products: &mut PhaseProducts,
            _signatures: &mut dyn SignatureProvider,
        ) -> Result<QueryOutput<AnalyzedBody>, QueryError> {
            panic!("declaration loading entered a body phase")
        }
    }
    impl LoweringPhase for NoBodies {
        fn lower(
            &self,
            _t: &hirsel_types::TypeInterner,
            _m: &hirsel_types::TypeMemo,
            _body: &AnalyzedBody,
            _comptime: &Comptime,
            _signature: &Signature,
        ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
            panic!("declaration loading entered a body phase")
        }
        fn lower_evaluation(
            &self,
            _t: &hirsel_types::TypeInterner,
            _m: &hirsel_types::TypeMemo,
            _body: &AnalyzedBody,
            _staged: &[(hirsel_syntax::ExprId, hirsel_types::TypeId)],
            _signature: &Signature,
            _expression: hirsel_syntax::ExprId,
            _result: hirsel_types::TypeId,
            _constants: &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)],
        ) -> Result<hirsel_tir::Definition, QueryError> {
            panic!("declaration loading entered a body phase")
        }
        fn entry_body(&self) -> hirsel_tir::BodyId {
            panic!("declaration loading entered a body phase")
        }
    }

    struct DeclarationPhases;
    impl Phases for DeclarationPhases {
        fn normalize_signature(
            &self,
            _: hirsel_check_protocol::header::Input<'_>,
            _: &mut dyn SignatureProvider,
        ) -> Result<hirsel_check_protocol::header::NormalizedDeclaration, QueryError> {
            panic!("declaration loading normalized a signature")
        }
        fn validate_loop_source(
            &self,
            _: &TypeInterner,
            _: &TypeMemo,
            _: &Signature,
            _: Span,
            _: &mut dyn SignatureProvider,
        ) -> Result<(), QueryError> {
            panic!("declaration loading validated an association")
        }
        fn validate_format(
            &self,
            _: &TypeInterner,
            _: &TypeMemo,
            _: &Signature,
            _: Span,
            _: &mut dyn SignatureProvider,
        ) -> Result<(), QueryError> {
            panic!("declaration loading validated an association")
        }
        fn validate_serializable_check(
            &self,
            _: &TypeInterner,
            _: &TypeMemo,
            _: &Signature,
            _: Span,
            _: &mut dyn SignatureProvider,
        ) -> Result<(), QueryError> {
            panic!("declaration loading validated an association")
        }
        fn checker(&self) -> Arc<dyn CheckerPhase> {
            Arc::new(NoBodies)
        }
        fn modes(&self) -> Arc<dyn ModesPhase> {
            Arc::new(NoBodies)
        }
        fn lowering(&self) -> Arc<dyn LoweringPhase> {
            Arc::new(NoBodies)
        }
    }
    fn phases() -> Arc<dyn Phases> {
        Arc::new(DeclarationPhases)
    }

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "declaration-policy-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            let fixture = Self(root);
            fixture.write("dep.hir", "pub fn unused() -> u64 { unknown() }\n");
            fixture.write("api.hir", "pub fn unused() -> u64 { let x: Missing = unknown(); x }\npub const CYCLE: u64 = CYCLE\npub fn generated() -> u64 = comptime missing_generator()\npub fn generic[T]() -> u64 = comptime missing_generator()\n");
            fixture.manifest(serde_json::json!([{
                "manifest": "dep.json", "package": {"name":"dep","version":"1"}, "expose": []
            }]));
            fixture.write("dep.json", r#"{"schema":1,"package":{"name":"dep","version":"1"},"sources":[{"path":"dep.hir","module":[]}],"dependencies":[]}"#);
            fixture
        }
        fn write(&self, path: &str, text: &str) {
            std::fs::write(self.0.join(path), text).unwrap();
        }
        fn manifest(&self, dependencies: serde_json::Value) {
            self.write(
                "hirsel.json",
                &serde_json::json!({
                    "schema":1, "package":{"name":"probe","version":"1"},
                    "sources":[{"path":"api.hir","module":[]}], "dependencies":dependencies
                })
                .to_string(),
            );
        }
        fn path(&self) -> std::path::PathBuf {
            self.0.join("hirsel.json")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn assert_only_declarations(package: &PackageDeclarations) {
        let engine = &package.engine;
        assert_eq!(
            package.files.len(),
            2,
            "root and dependency indexed without an entry"
        );
        assert_eq!(engine.work_stats().parsed_definitions, 5);
        assert!(
            engine
                .memo_stats()
                .recomputed
                .keys()
                .all(|kind| matches!(kind, QueryKind::Lex | QueryKind::Parse))
        );
        assert!(engine.constant_values.is_empty());
        assert!(engine.staged_values.is_empty());
        assert!(engine.staged_constants.is_empty());
        assert_eq!(engine.generated_sources().count(), 0);
        assert!(engine.last_comptime_limit.is_none());
        for (_, context) in &package.files {
            assert!(!engine.imaged(&context.package));
        }
    }

    #[test]
    fn declaration_loading_preserves_verify_and_defaults_without_bodies() {
        let fixture = Fixture::new();
        let compiler = Compiler::new(phases, [17; 32]);
        assert_eq!(compiler.verify(), Verify::On);
        for verify in [Verify::On, Verify::Off] {
            let package = compiler
                .clone()
                .with_verify(verify)
                .load_package_declarations(&fixture.path())
                .unwrap();
            assert_eq!(package.engine.verify, verify);
            assert_only_declarations(&package);
            let (ordinary, _) = compiler
                .clone()
                .with_verify(verify)
                .load(&fixture.0.join("api.hir"))
                .unwrap();
            assert_eq!(ordinary.verify, verify);
        }
    }

    #[test]
    fn declaration_loading_preserves_all_configured_session_policies() {
        let fixture = Fixture::new();
        let store = Arc::new(crate::store::SimStore::default());
        let limits = hirsel_interp::Limits {
            steps: Some(0),
            call_depth: 1,
        };
        for owned in [false, true] {
            let compiler = Compiler::new(phases, [23; 32])
                .with_config(CompileConfig {
                    jobs: hirsel_intern::jobs::Jobs::new(3),
                    verify: Verify::Off,
                    retention: crate::memo::Retention::Resident,
                    lowering: LoweringPolicy::Reachable,
                    store: store.clone(),
                    verify_store: crate::store::VerifyStore::Recompute,
                    hold_bytes: 0,
                })
                .with_comptime_limits(limits);
            let compiler = if owned {
                compiler.with_owned_engines()
            } else {
                compiler
            };
            let package = compiler.load_package_declarations(&fixture.path()).unwrap();
            let engine = &package.engine;
            assert_eq!(engine.verify, Verify::Off);
            assert_eq!(engine.retention, crate::memo::Retention::Resident);
            assert_eq!(engine.configured_jobs.unwrap().get(), 3);
            assert_eq!(engine.configured_lowering, LoweringPolicy::Reachable);
            assert_eq!(engine.hold_bytes(), 0);
            assert_eq!(engine.verify_store, crate::store::VerifyStore::Recompute);
            let provider: Arc<dyn crate::store::ResultStore> = store.clone();
            assert!(Arc::ptr_eq(
                engine.result_store.as_ref().unwrap(),
                &provider
            ));
            assert_eq!(engine.comptime_limits, limits);
            assert_eq!(engine.session, !owned);
            assert_eq!(engine.store_identity, ContentHash([23; 32]));
            assert_only_declarations(&package);
        }
        assert!(
            store.log().is_empty(),
            "indexing must not query or populate the result store"
        );
    }

    #[test]
    fn declaration_loading_still_refuses_invalid_manifests_and_dependencies() {
        let fixture = Fixture::new();
        for verify in [Verify::On, Verify::Off] {
            let compiler = Compiler::new(phases, [0; 32]).with_verify(verify);
            fixture.write("hirsel.json", "{");
            assert!(compiler.load_package_declarations(&fixture.path()).is_err());
            fixture.manifest(serde_json::json!([{
                "manifest":"missing.json", "package":{"name":"dep","version":"1"}, "expose":[]
            }]));
            assert!(compiler.load_package_declarations(&fixture.path()).is_err());
            fixture.manifest(serde_json::json!([{
                "manifest":"dep.json", "package":{"name":"dep","version":"2"}, "expose":[]
            }]));
            let error = compiler
                .load_package_declarations(&fixture.path())
                .err()
                .unwrap();
            assert!(
                error
                    .to_string()
                    .contains("selected package version does not match dependency")
            );
        }
    }
}
