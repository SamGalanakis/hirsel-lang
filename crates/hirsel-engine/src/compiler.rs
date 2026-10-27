//! A complete compiler: the query engine over one set of front phases. Path
//! entry points start fresh sessions on the compiler thread, so a compiler is
//! the plain constructor of its phases and crosses threads freely.
use crate::*;
use std::{io::Write, path::Path, sync::Arc};

/// A fully prepared source; constructing a batch never writes to disk.
pub struct FormattedSource {
    pub path: std::path::PathBuf,
    pub original: String,
    pub formatted: String,
}
impl FormattedSource {
    pub fn changed(&self) -> bool {
        self.original != self.formatted
    }
}

#[derive(Clone)]
pub struct Compiler {
    owned_engines: bool,
    adapter_factory: Option<fn() -> PhaseAdapters>,
    phases: fn() -> Arc<dyn Phases>,
    /// Digest of the compiler's own sources, bound by ordinary query products.
    identity: [u8; 32],
    /// Whether the sessions this compiler starts run V.
    verify: Verify,
    retention: crate::memo::Retention,
    jobs: Option<hirsel_intern::jobs::Jobs>,
    hold_bytes: Option<u64>,
    lowering: LoweringPolicy,
    result_store: Option<Arc<dyn crate::store::ResultStore>>,
    verify_store: crate::store::VerifyStore,
    comptime_limits: Option<crate::ComptimeLimits>,
}
impl Compiler {
    /// A compiler whose sessions run V.
    pub const fn new(phases: fn() -> Arc<dyn Phases>, identity: [u8; 32]) -> Self {
        Self {
            owned_engines: false,
            adapter_factory: None,
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
    /// Capture the ordinary host defaults once for an immutable request host.
    /// An explicitly configured provider (including none@1) takes precedence.
    pub fn configuration(&self) -> CompileConfig {
        let mut config = CompileConfig::default();
        config.verify = self.verify;
        config.retention = self.retention;
        config.lowering = self.lowering;
        config.jobs = self.jobs.unwrap_or(config.jobs);
        config.hold_bytes = self.hold_bytes.unwrap_or(config.hold_bytes);
        config.verify_store = self.verify_store;
        if let Some(store) = &self.result_store {
            config.store = store.clone();
        } else if crate::policy::uses_persistent_results() {
            let name = std::env::var("HIRSEL_RESULTS").unwrap_or_else(|_| "dir@1".into());
            if crate::policy::verifies_cached_results()
                || std::env::var_os("HIRSEL_VERIFY_CACHE").is_some_and(|v| v != "0")
            {
                config.verify_store = crate::store::VerifyStore::Recompute;
            }
            if let Ok(store) = crate::store::store_by_name(
                &name,
                std::env::var_os("HIRSEL_RESULTS_DIR")
                    .filter(|v| !v.is_empty())
                    .map(Into::into),
            ) {
                config.store = store;
            }
        }
        config
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
    /// Compile-time limits are explicit query inputs.
    pub const fn with_comptime_limits(mut self, limits: crate::ComptimeLimits) -> Self {
        self.comptime_limits = Some(limits);
        self
    }
    pub(crate) fn comptime_limits(&self) -> crate::ComptimeLimits {
        self.comptime_limits.unwrap_or_default()
    }
    /// A fresh session engine of this compiler.
    pub(crate) fn engine(&self, environment: QueryEnvironment) -> QueryEngine {
        let hold_bytes = self
            .hold_bytes
            .unwrap_or_else(crate::config::configured_hold_bytes);
        let mut engine = QueryEngine::new_with_hold_bytes(environment, self.adapters(), hold_bytes);
        engine.verify = self.verify;
        engine.set_retention(self.retention);
        engine.configured_jobs = self.jobs;
        engine.configured_lowering = self.lowering;
        engine.set_hold_bytes(hold_bytes);
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
    /// Select an explicit phase factory and its compiler identity before any
    /// snapshot is loaded. Alternate adapters cannot reuse ordinary products
    /// under the ordinary compiler identity.
    pub fn with_phase_adapters(
        mut self,
        factory: fn() -> PhaseAdapters,
        identity: [u8; 32],
    ) -> Self {
        self.adapter_factory = Some(factory);
        self.identity = identity;
        self
    }
    /// A fresh complete adapter set with immutable shared front phases.
    pub fn adapters(&self) -> PhaseAdapters {
        self.adapter_factory.map_or_else(
            || PhaseAdapters::compiler((self.phases)()),
            |factory| factory(),
        )
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
    /// or any body checking or compile-time evaluation. The
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
        crate::package::load_program_source(
            self,
            path,
            replacement,
            crate::package::EntrySelection::Launch,
        )
    }
    /// Load exclusively from explicit input observations. Discovery closes the package/input set without source parsing,
    /// so a syntax-error base can still be repaired. Manifest errors remain
    /// loader errors. Query products use the configured ordinary provider.
    pub fn load_program_inputs(
        &self,
        path: &Path,
        inputs: &dyn crate::loader_inputs::LoaderInputs,
        parse_sources: bool,
    ) -> Result<LoadedProgram, ToolError> {
        crate::package::load_program_inputs(
            self,
            path,
            inputs,
            crate::package::Loading {
                replacement: None,
                entry_selection: crate::package::EntrySelection::Launch,
                parse_sources,
            },
        )
    }
    /// Load the selected launch program for checked declaration/layout export.
    /// Manifest and source validation are unchanged; foreign link files may be
    /// produced later by compiling against the exported declarations.
    pub fn load_export_package(&self, path: &Path) -> Result<LoadedProgram, ToolError> {
        crate::package::load_program_source(
            self,
            path,
            None,
            crate::package::EntrySelection::Export,
        )
    }
    /// Observe declaration exports before foreign link artifacts are produced.
    pub fn load_export_package_inputs(
        &self,
        path: &Path,
        inputs: &dyn crate::loader_inputs::LoaderInputs,
    ) -> Result<LoadedProgram, ToolError> {
        crate::package::load_program_inputs(
            self,
            path,
            inputs,
            crate::package::Loading {
                replacement: None,
                entry_selection: crate::package::EntrySelection::Export,
                parse_sources: true,
            },
        )
    }
    /// Load sources for checking, accepting a package with no launch entry.
    pub fn load_check_package(&self, path: &Path) -> Result<LoadedProgram, ToolError> {
        crate::package::load_program_source(self, path, None, crate::package::EntrySelection::Check)
    }
    /// Discover or load a check package from the same immutable observations as
    /// launch requests, accepting libraries without a launch entry.
    pub fn load_check_package_inputs(
        &self,
        path: &Path,
        inputs: &dyn crate::loader_inputs::LoaderInputs,
        parse_sources: bool,
    ) -> Result<LoadedProgram, ToolError> {
        crate::package::load_program_inputs(
            self,
            path,
            inputs,
            crate::package::Loading {
                replacement: None,
                entry_selection: crate::package::EntrySelection::Check,
                parse_sources,
            },
        )
    }
    /// Load a fresh immutable request using admitted source/header indexes from
    /// a prior request. Body products and controllers are never restored.
    pub fn load_check_package_inputs_with_source_index(
        &self,
        path: &Path,
        inputs: &dyn crate::loader_inputs::LoaderInputs,
        source_index: crate::SourceIndexState,
    ) -> Result<LoadedProgram, ToolError> {
        crate::package::load_program_inputs_with_source_index(
            self,
            path,
            inputs,
            crate::package::Loading {
                replacement: None,
                entry_selection: crate::package::EntrySelection::Check,
                parse_sources: true,
            },
            Some(source_index),
        )
    }
    /// Load an explicit test package without requiring a launch entry. The root
    /// package's sources are checked in full; no function is selected as main.
    pub fn load_test_package(&self, manifest: &Path) -> Result<LoadedProgram, ToolError> {
        crate::package::load_program_source(
            self,
            manifest,
            None,
            crate::package::EntrySelection::Test,
        )
    }
    /// Load the same test package from one observed input snapshot.
    pub fn load_test_package_inputs(
        &self,
        path: &Path,
        inputs: &dyn crate::loader_inputs::LoaderInputs,
    ) -> Result<LoadedProgram, ToolError> {
        crate::package::load_program_inputs(
            self,
            path,
            inputs,
            crate::package::Loading {
                replacement: None,
                entry_selection: crate::package::EntrySelection::Test,
                parse_sources: true,
            },
        )
    }
    /// Precompute a package through ordinary query persistence. Every product
    /// remains independently keyed and checked; there is no package image or
    /// whole-program type table. Partial work is a set of ordinary cache misses.
    pub fn precompute_package(&self, manifest: &Path, output: &Path) -> Result<(), ToolError> {
        if self.verify == Verify::Off {
            return Err(ToolError::Runtime(
                "package precomputation requires V".into(),
            ));
        }
        let mut compiler = self.clone().with_owned_engines();
        let store = Arc::new(crate::store::DirStore::new(output.to_path_buf()));
        // Own all encoding, transitive type and V writes before package load.
        let publication =
            crate::store::ResultStore::local_product_store(&*store).map(|store| store.request());
        compiler.result_store = Some(store);
        compiler.retention = crate::memo::Retention::Batch;
        compiler.hold_bytes = Some(0);
        let result = with_compiler_stack(|| {
            let mut program = crate::package::load_program_source(
                &compiler,
                manifest,
                None,
                crate::package::EntrySelection::Check,
            )?;
            let checked = program
                .check(LoweringPolicy::Reachable)
                .map_err(|error| program.error(error))?;
            if crate::queries::has_errors(&checked.diagnostics) {
                return Err(program.error(QueryError::Diagnostics(checked.diagnostics)));
            }
            if checked.completion != CheckCompletion::Complete {
                return Err(program.error(QueryError::NotImplemented));
            }
            // Each explicit demand releases its intermediate front body under
            // Batch retention. No second program or encoded package accumulates.
            for path in program.engine.source_paths() {
                let facts = program.engine.source_facts(&path);
                program
                    .engine
                    .semantic_node(&path)
                    .map_err(|error| program.error(error))?;
                if facts.as_ref().is_some_and(|facts| facts.executable) {
                    program
                        .engine
                        .source_audit(&path)
                        .map_err(|error| program.error(error))?;
                    program
                        .engine
                        .lower_to_tir(&path)
                        .map_err(|error| program.error(error))?;
                }
                if facts.is_some_and(|facts| facts.constant) {
                    program
                        .engine
                        .precompute_constant(&path)
                        .map_err(|error| program.error(error))?;
                }
            }
            Ok(())
        });
        result?;
        if let Some(publication) = publication {
            publication
                .finish()
                .map_err(|e| ToolError::Runtime(e.to_string()))?;
        }
        Ok(())
    }
    /// Format one source, preserving the single-source API's output.
    pub fn format_path(&self, path: &Path) -> Result<String, ToolError> {
        self.prepare_format_source(path)
            .map(|source| source.formatted)
    }
    /// Read and format every direct source before returning any output. A
    /// manifest selects its explicit sources in order, deduplicated by canonical
    /// path; a source selects only itself, without consulting its sidecar.
    pub fn prepare_format_path(&self, path: &Path) -> Result<Vec<FormattedSource>, ToolError> {
        crate::package::format_sources(path)?
            .iter()
            .map(|path| self.prepare_format_source(path))
            .collect()
    }
    fn prepare_format_source(&self, path: &Path) -> Result<FormattedSource, ToolError> {
        with_compiler_stack(|| {
            let (mut engine, file) = self.load(path)?;
            let original = engine.sources.file(file).unwrap().text().to_owned();
            let formatted =
                format_snapshot(&mut engine, file, &default_context()).map_err(|error| {
                    ToolError::Query {
                        error,
                        phase: engine.unavailable,
                        sources: engine.sources.clone(),
                    }
                })?;
            Ok(FormattedSource {
                path: path.to_path_buf(),
                original,
                formatted,
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

#[cfg(test)]
pub(crate) mod tests {
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
    pub(crate) fn phases() -> Arc<dyn Phases> {
        Arc::new(DeclarationPhases)
    }

    #[test]
    fn request_host_captures_explicit_policy_and_adapter_identity() {
        struct Unavailable;
        impl LexerPhase for Unavailable {
            fn lex(&self, _: FileId, _: &SourceMap) -> Result<QueryOutput<Vec<Token>>, QueryError> {
                Err(QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "request-lexer".into(),
                    message: "disconnected".into(),
                }))
            }
        }
        fn adapters() -> PhaseAdapters {
            let mut adapters = PhaseAdapters::compiler(phases());
            adapters.lexer = Some(Arc::new(Unavailable));
            adapters
        }
        with_compiler_stack(|| {
            let store: Arc<dyn crate::store::ResultStore> =
                Arc::new(crate::store::MemStore::default());
            let config = CompileConfig {
                jobs: hirsel_intern::jobs::Jobs::new(8),
                verify: Verify::Off,
                retention: crate::memo::Retention::Resident,
                lowering: LoweringPolicy::Reachable,
                hold_bytes: 0,
                store: store.clone(),
                verify_store: crate::store::VerifyStore::Recompute,
            };
            let compiler = Compiler::new(phases, [1; 32])
                .with_config(config)
                .with_phase_adapters(adapters, [2; 32])
                .with_owned_engines();
            let captured = compiler.configuration();
            assert!(Arc::ptr_eq(&captured.store, &store));
            assert_eq!(captured.jobs.get(), 8);
            assert_eq!(captured.verify, Verify::Off);
            assert_eq!(captured.retention, crate::memo::Retention::Resident);
            assert_eq!(captured.lowering, LoweringPolicy::Reachable);
            assert_eq!(captured.hold_bytes, 0);
            assert_eq!(captured.verify_store, crate::store::VerifyStore::Recompute);
            let mut engine = compiler.engine(default_environment());
            assert_eq!(engine.store_identity, ContentHash([2; 32]));
            let file = engine
                .sources
                .add("request-lexer.hir", "fn main() {}".into())
                .unwrap();
            let error = engine.parse_file(file, &default_context()).unwrap_err();
            assert_eq!(
                error,
                QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "request-lexer".into(),
                    message: "disconnected".into(),
                })
            );
        });
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
        let limits = crate::ComptimeLimits {
            budget: 0,
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
