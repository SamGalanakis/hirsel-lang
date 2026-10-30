//! Per-definition queries and phase adapters (LANG-6, PROG-5).

#[cfg(test)]
hirsel_guard::install!(test_alloc::Counting: test_alloc::Counting);

#[cfg(test)]
mod test_alloc;
mod work;
pub use work::WorkStats;
mod operations;
pub use operations::*;
mod native_audit;
pub use native_audit::{CleanupSite, SourceAudit};
mod native_semantic;
pub use native_semantic::NativeSemanticProgram;
mod native_metadata;
pub use native_metadata::NativeMetadataWeight;
mod native_queries;
pub use hirsel_product_codec::{CheckedTir, TirBlockView, TirBodyView, TirMatchView, TirView};
pub use native_queries::*;
mod native_lower;
pub use native_lower::{
    NATIVE_ORIGINAL_BODY, NativeLowerBody, NativeLowerReader, NativeOriginalBody,
    NativeOriginalContext, NativeOriginalProduct, NativeOriginalReader, NativeOriginalValue,
    NativeScalarLower,
};
mod native_input;
pub use native_input::*;
mod representation;
pub use representation::RepresentationRecord;
mod compiler;
mod comptime;
pub use comptime::{ComptimeLimits, DEFAULT_COMPTIME_BUDGET};
mod config;
pub use compiler::{Compiler, FormattedSource};
pub use config::CompileConfig;
mod dictionaries;
mod families;
mod materialized;
mod phases;
pub mod registry;
pub use phases::*;
mod diagnostics;
pub mod memo;
pub mod store;
pub use memo::{Durability, ReadKey, Reads, ReadsBuilder, Revision};
mod generic_graph;
mod storage_graph;
mod table_keys;
pub use diagnostics::*;
pub use generic_graph::{InstanceEdge, InstanceGraph, InstanceKind};
mod source_components;
mod summary;
pub use source_components::{
    SourceGrowthComponent, SourceGrowthFacets, SourceGrowthFact, SourceGrowthProducer,
};
pub use summary::{
    CallClass, CheckedSummary, DefinitionSummary, Frame, FrameKind, GenericParameterRef,
    GrowthEdge, ParameterGrowth, PublishedSummary, SummaryCall, SummaryView, growing_sites,
    summarize,
};
mod semantic;
mod snapshot;
pub use semantic::*;
pub use snapshot::{
    CitationSource, DeclarationIndex, Declarations, FrozenProvider, ModuleInputs, PackageViews,
    SignatureSnapshot, View, ViewTable,
};
mod adapters;
mod facts;
pub use adapters::*;
pub mod loader_inputs;
mod package;
pub mod requests;
pub use store::release_shared_caches;
mod policy;
pub use policy::{
    LoweringPolicy, Verify, free_sessions, persistent_results, verify_cached_results,
};
mod admission;
mod proof_export;
mod queries;
mod tools;
pub use admission::{
    admit_run, bind_run, callee_facts, entry_facts, entry_launch_kind, entry_roots,
    read_run_record, served_entries,
};
use hirsel_diag::{Diagnostic, RuleId};
use hirsel_intern::Symbol;
pub use hirsel_interp::library::SimulationConfig;
use hirsel_span::{FileId, SourceMap};
pub use hirsel_syntax::FileContext;
use hirsel_syntax::{ContentHash, ItemSyntax, SyntaxFile, Token};
pub use hirsel_syntax::{DefId, DefPath};
pub use hirsel_tir::{CapabilityEvent, RunConfiguration, TirProvider};
use hirsel_types::{CheckedBody, Resolution, Signature};
pub use hirsel_types::{QueryError, QueryOutput, SignatureDependency, SignatureProvider};
pub use package::*;
pub use queries::native_records::{NativeRecordContext, NativeRecordSource, NativeRecords};
pub use queries::*;
use std::{fmt, io::Write};
pub use tools::*;

/// A phase is absent, not a language rejection (PROG-5).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NotImplemented;
impl fmt::Display for NotImplemented {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not implemented yet")
    }
}
impl std::error::Error for NotImplemented {}
pub use hirsel_run_schema::RunOutcome;
/// Rich execution export. Execution API errors retain output beside their cause.
#[derive(Debug)]
pub struct ObservedExecution {
    /// Reference tooling accounting, outside semantic observations and the Io tape.
    /// Absent when execution could not produce an accounting result.
    pub step_usage: Option<StepUsage>,
    /// Simulator tooling, outside semantic observations.
    pub race_candidates: Vec<hirsel_interp::library::race::RaceCandidate>,
    pub outcome: Result<RunOutcome, RuntimeError>,
    pub observation: Option<hirsel_interp::observation::Observable>,
    /// Output remains available without a semantic observation, including API errors.
    pub tooling_output: Vec<u8>,
    pub tooling_stderr: Vec<u8>,
    /// PROG-3 host text and its position, outside semantic observations and Io tape.
    pub exit_report: Option<hirsel_interp::exit_report::Report>,
}
/// The requested interpreter policy and the actual number of charged TIR steps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StepUsage {
    pub limit: Option<u64>,
    pub consumed: u64,
}
/// Diagnostics own their source snapshot, so locations cannot silently drift (FAIL-1).
#[derive(Clone, Debug, Default)]
pub struct CheckOutput {
    pub completion: CheckCompletion,
    pub sources: SourceMap,
    pub diagnostics: Vec<Diagnostic>,
    /// Every selected package marked `unsafe ffi`, which `hirsel check` reports (PROG-4).
    pub unsafe_ffi: Vec<UnsafeFfiPackage>,
}
/// A selected package's `unsafe ffi` manifest metadata (PROG-4).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnsafeFfiPackage {
    pub package: hirsel_syntax::PackageId,
    /// Native link inputs in manifest order.
    pub link: Vec<LinkInput>,
    /// Trusted and reported, never verified.
    pub headers_checked: bool,
    pub wrappers: std::collections::BTreeMap<DefPath, hirsel_types::ForeignWrapper>,
    /// Where its externs run (S-13).
    pub threads: hirsel_types::ForeignThreads,
}
/// One native library an `unsafe ffi` package links against.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinkInput {
    /// `-l<name>`.
    Library(String),
    /// An archive, object or shared library, canonical.
    File(std::path::PathBuf),
}
/// Completion is per check, carried beside all diagnostics obtained so far.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CheckCompletion {
    #[default]
    Complete,
    Incomplete {
        phase: QueryKind,
    },
}
/// Cache domains keep outputs of different phases distinct (PROG-5).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub enum QueryKind {
    Lex,
    Format,
    Interpret,
    Parse,
    Resolve,
    Signature,
    Modes,
    Check,
    Lower,
    Summary,
    CheckedSummary,
    Registration,
    Evaluate,
    Generate,
    GeneratedRegistration,
    SourceAudit,
    SemanticBody,
    PrivateInference,
}
impl QueryKind {
    pub fn phase_name(self) -> &'static str {
        match self {
            Self::Lex => "lexing",
            Self::Parse => "parsing",
            Self::Resolve => "name resolution",
            Self::Signature => "signature normalization",
            Self::Check => "body checking",
            Self::Modes => "mode checking",
            Self::Lower => "TIR lowering",
            Self::Format => "formatting",
            Self::Interpret => "interpretation",
            Self::Summary => "definition summary",
            Self::CheckedSummary => "checked definition summary",
            Self::Registration => "staged declaration registration",
            Self::Evaluate => "compile-time evaluation",
            Self::Generate => "generated source evaluation",
            Self::GeneratedRegistration => "generated instance registration",
            Self::SourceAudit => "source allocation audit",
            Self::SemanticBody => "semantic source body",
            Self::PrivateInference => "private uniqueness inference",
        }
    }
}
/// All non-source meaning inputs are explicit; topology is intentionally absent (DET-3, PROG-5).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryEnvironment {
    pub interface_version: u32,
    pub compiler_version: String,
    pub spec_hash: ContentHash,
    pub package_manifest_hash: ContentHash,
    pub builtin_signatures_hash: ContentHash,
}
/// A content key; whitespace/comments affect text hashes, positions do not affect DefPath (PROG-5).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionKey {
    pub kind: QueryKind,
    pub definition: DefPath,
    pub text_hash: ContentHash,
    pub dependencies: Vec<SignatureDependency>,
    pub environment: QueryEnvironment,
}
impl DefinitionKey {
    /// Reject conflicting observations of a dependency; duplicate equal reads are harmless.
    pub fn fingerprint(&self) -> Result<ContentHash, &'static str> {
        let mut deps = self.dependencies.clone();
        deps.sort_by_key(|a| a.definition);
        if deps
            .windows(2)
            .any(|p| p[0].definition == p[1].definition && p[0].fingerprint != p[1].fingerprint)
        {
            return Err("dependency changed within query snapshot");
        }
        deps.dedup();
        let kind = match self.kind {
            QueryKind::Format => 7,
            QueryKind::Interpret => 8,
            QueryKind::Registration => 11,
            QueryKind::Evaluate => 12,
            QueryKind::Generate => 13,
            QueryKind::GeneratedRegistration => 14,
            QueryKind::SourceAudit => 15,
            QueryKind::SemanticBody => 16,
            QueryKind::PrivateInference => 17,
            QueryKind::Lex => 4,
            QueryKind::Resolve => 5,
            QueryKind::Modes => 6,
            QueryKind::Parse => 0,
            QueryKind::Signature => 1,
            QueryKind::Check => 2,
            QueryKind::Lower => 3,
            QueryKind::Summary => 9,
            QueryKind::CheckedSummary => 10,
        };
        let mut parts = vec![
            vec![kind],
            self.definition.fingerprint().0.to_vec(),
            self.text_hash.0.to_vec(),
            self.environment.interface_version.to_le_bytes().to_vec(),
            self.environment.compiler_version.as_bytes().to_vec(),
            self.environment.spec_hash.0.to_vec(),
            self.environment.package_manifest_hash.0.to_vec(),
            self.environment.builtin_signatures_hash.0.to_vec(),
        ];
        for dep in deps {
            parts.push(dep.definition.fingerprint().0.to_vec());
            parts.push(dep.fingerprint.0.to_vec());
        }
        Ok(ContentHash::of_parts(
            "hirsel.query.v12",
            parts.iter().map(Vec::as_slice),
        ))
    }
}
/// Stable orchestration surface. Implementations record every signature query, including contracts (LANG-6).
/// QueryEngine supplies caching over coherent immutable source/interner snapshots.
pub trait DefinitionQueries {
    fn parse_file(
        &mut self,
        file: FileId,
        context: &FileContext,
    ) -> Result<QueryOutput<SyntaxFile>, QueryError>;
    fn item_signature(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<hirsel_types::SignatureRef>, QueryError>;
    fn check_definition(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<TypedBody>, QueryError>;
    fn lower_to_tir(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<hirsel_tir::DefinitionRef>, QueryError>;
}
/// Shared leaf phase signatures; implementations may expose equivalent free functions (LANG-1).
/// These traits live in the driver boundary; implementation crates need only the data crates.
pub trait LexerPhase: Send + Sync {
    fn lex(&self, file: FileId, sources: &SourceMap)
    -> Result<QueryOutput<Vec<Token>>, QueryError>;
}
/// Parsing receives tokens and file naming context; it does not resolve types
/// (LANG-1/2). It places one anchor per item in `sources`.
pub trait ParserPhase: Send + Sync {
    fn parse(
        &self,
        file: FileId,
        tokens: Vec<Token>,
        context: &FileContext,
        sources: &mut SourceMap,
    ) -> Result<QueryOutput<SyntaxFile>, QueryError>;
}
/// Formatting uses the complete token tape including error recovery and comments (LANG-1).
pub trait FormatterPhase: Send + Sync {
    fn format(&self, file: &SyntaxFile, sources: &SourceMap) -> Result<String, Vec<Diagnostic>>;
    /// Additive driver wrapper: adapters can distinguish conservative fallback from success.
    fn format_query(&self, file: &SyntaxFile, sources: &SourceMap) -> Result<String, QueryError> {
        self.format(file, sources).map_err(QueryError::Diagnostics)
    }
}
/// Names are resolved against signatures, never implementation internals (LANG-4/6).
pub trait ResolverPhase: Send + Sync {
    fn resolve(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        syntax: std::sync::Arc<ItemSyntax>,
        declarations: &Declarations,
        products: &mut PhaseProducts,
        signatures: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Resolution>, QueryError>;
}
/// Type checking produces body tables; modes then enrich/check them before lowering (LANG-6, MODE-7).
pub trait CheckerPhase: Send + Sync {
    fn check(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        syntax: std::sync::Arc<ItemSyntax>,
        resolution: Resolution,
        signature: &Signature,
        signatures: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<TypedBody>, QueryError>;
    /// GEN-6: whether `actuals` match the signature `family`, through the one
    /// matcher implementation declarations use. `Some` is the refusal, naming
    /// its member and rule. A checker without the matcher cannot decide.
    fn match_implementation(
        &self,
        _t: &hirsel_types::TypeInterner,
        _m: &hirsel_types::TypeMemo,
        _family: &DefPath,
        _actuals: &hirsel_types::ImplementationSignature,
        _signatures: &mut dyn SignatureProvider,
    ) -> Result<Option<String>, QueryError> {
        Err(QueryError::NotImplemented)
    }
}
/// Modes consume the complete typed artifact and refine its shared tables (OWN-1/8).
pub trait ModesPhase: Send + Sync {
    fn infers_private_unique(&self) -> bool {
        false
    }
    /// Greatest fixed point over the supplied same-module direct-call graph.
    fn infer_unique(
        &self,
        _t: &hirsel_types::TypeInterner,
        _m: &hirsel_types::TypeMemo,
        _bodies: &[(std::sync::Arc<TypedBody>, hirsel_types::SignatureRef)],
        _provider: &mut dyn SignatureProvider,
    ) -> Result<Vec<(DefId, hirsel_types::UniqueInference)>, QueryError> {
        Ok(vec![])
    }

    /// Demand-driven source equations after modes have completed. This is
    /// separate from signature lookup and from per-body checking.
    fn storage(
        &self,
        _t: &hirsel_types::TypeInterner,
        _m: &hirsel_types::TypeMemo,
        _body: &AnalyzedBody,
        _signature: &Signature,
        _signatures: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<hirsel_types::storage::graph::StorageDefinition>, QueryError> {
        Err(QueryError::NotImplemented)
    }
    fn check_modes(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        body: std::sync::Arc<TypedBody>,
        signature: &Signature,
        products: &mut PhaseProducts,
        signatures: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<AnalyzedBody>, QueryError>;
}
/// Lowering makes evaluation order and cleanup explicit before execution (LANG-3, DROP-2).
pub trait LoweringPhase: Send + Sync {
    fn lower(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        body: &AnalyzedBody,
        comptime: &Comptime,
        signature: &Signature,
    ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError>;
    /// Lowers one checked subexpression as a zero-input compile-time initializer
    /// whose result is `result`; `constants` are its already materialized parts.
    fn lower_evaluation(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        body: &AnalyzedBody,
        staged: &[(hirsel_syntax::ExprId, hirsel_types::TypeId)],
        signature: &Signature,
        expression: hirsel_syntax::ExprId,
        result: hirsel_types::TypeId,
        constants: &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)],
    ) -> Result<hirsel_tir::Definition, QueryError>;
    /// The body every lowered definition enters through.
    fn entry_body(&self) -> hirsel_tir::BodyId;
}
/// Runtime output includes failure cause data; stdout is delivered as exact bytes (FAIL-2; issue #12).
pub trait InterpreterPhase: Send {
    /// Consume only the most recent run's separately requested resource diagnostics.
    fn take_resource_trace(
        &mut self,
    ) -> Option<
        Result<
            hirsel_run_schema::ownership_trace::Trace,
            hirsel_interp::ownership_trace::TraceError,
        >,
    > {
        None
    }
    /// Replace only the tooling step bound, returning the previous policy.
    /// Adapters without reference step accounting refuse bounded execution.
    fn replace_step_limit(&mut self, _limit: Option<u64>) -> Result<Option<u64>, NotImplemented> {
        Err(NotImplemented)
    }

    fn prepare(
        &mut self,
        _signature: hirsel_types::SignatureRef,
        _runtime: hirsel_interp::aggregate::RuntimeTypes,
        _simulation: hirsel_interp::library::SimulationConfig,
    ) {
    }
    fn run(
        &mut self,
        config: &RunConfiguration,
        definitions: &mut dyn TirProvider,
        stdout: &mut dyn Write,
    ) -> Result<RunOutcome, QueryError>;
    /// The implementation API additionally needs the owning source snapshot and tool errors.
    fn run_observed_query(
        &mut self,
        _config: &RunConfiguration,
        _definitions: &mut dyn TirProvider,
        _sources: &SourceMap,
    ) -> Result<ObservedExecution, RuntimeError> {
        Err(RuntimeError::Query(QueryError::NotImplemented))
    }
    fn run_query(
        &mut self,
        config: &RunConfiguration,
        definitions: &mut dyn TirProvider,
        stdout: &mut dyn Write,
        _sources: &SourceMap,
    ) -> Result<RunOutcome, RuntimeError> {
        self.run(config, definitions, stdout)
            .map_err(RuntimeError::Query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_syntax::{ItemPath, PackageId};
    fn path(name: &str) -> DefPath {
        DefPath(ItemPath {
            package: PackageId {
                name: "p".into(),
                version: "1".into(),
            },
            modules: vec![],
            name: name.into(),
        })
    }
    #[test]
    fn signature_dependency_wire_stays_path_based_inside_image_scopes() {
        use hirsel_intern::persist::{Numbering, Source};
        use hirsel_syntax::{DefId, with_def_numbering, with_def_source};
        #[derive(serde::Serialize, serde::Deserialize)]
        struct LegacyDependency {
            definition: DefPath,
            fingerprint: ContentHash,
        }
        struct NoDefinitions;
        impl Source<DefId> for NoDefinitions {
            fn id(&self, _: u32) -> Option<DefId> {
                panic!("a signature dependency must decode its path directly")
            }
        }
        let paths = [path("z"), path("a"), DefPath::core("nested")];
        // Intern in reverse canonical order; neither wire encoding may expose it.
        for path in &paths {
            DefId::intern(path);
        }
        for path in paths {
            let fingerprint = ContentHash::of_bytes(path.to_string().as_bytes());
            let legacy = LegacyDependency {
                definition: path.clone(),
                fingerprint,
            };
            let dependency = SignatureDependency {
                definition: DefId::intern(&path),
                fingerprint,
            };
            let old_json = serde_json::to_vec(&legacy).unwrap();
            let old_binary = bincode::serialize(&legacy).unwrap();
            assert_eq!(serde_json::to_vec(&dependency).unwrap(), old_json);
            assert_eq!(bincode::serialize(&dependency).unwrap(), old_binary);
            let mut numbering = Numbering::default();
            with_def_numbering(&mut numbering, || {
                assert_eq!(serde_json::to_vec(&dependency).unwrap(), old_json);
                assert_eq!(bincode::serialize(&dependency).unwrap(), old_binary);
            });
            assert!(
                numbering.ids().is_empty(),
                "dependency paths never use the image id table"
            );
            with_def_source(std::sync::Arc::new(NoDefinitions), || {
                assert_eq!(
                    serde_json::from_slice::<SignatureDependency>(&old_json).unwrap(),
                    dependency
                );
                assert_eq!(
                    bincode::deserialize::<SignatureDependency>(&old_binary).unwrap(),
                    dependency
                );
            });
            assert_eq!(
                serde_json::from_slice::<SignatureDependency>(&old_json).unwrap(),
                dependency
            );
            assert_eq!(
                bincode::deserialize::<SignatureDependency>(&old_binary).unwrap(),
                dependency
            );
        }
    }
    #[test]
    fn signature_read_wire_keeps_legacy_enum_tag_and_path_inside_image_scopes() {
        use hirsel_intern::persist::{Numbering, Source};
        use hirsel_syntax::{DefId, with_def_numbering, with_def_source};
        use hirsel_types::DependencyRead;
        struct NoDefinitions;
        impl Source<DefId> for NoDefinitions {
            fn id(&self, _: u32) -> Option<DefId> {
                panic!("a signature read must decode its path directly")
            }
        }
        for path in [path("z"), path("a"), DefPath::core(&"long".repeat(1024))] {
            for fingerprint in [None, Some(ContentHash::of_bytes(b"observed"))] {
                let read = DependencyRead::Signature {
                    definition: DefId::intern(&path),
                    fingerprint,
                }
                .shared();
                let json = format!(
                    "{{\"Signature\":{{\"definition\":{},\"fingerprint\":{}}}}}",
                    serde_json::to_string(&path).unwrap(),
                    serde_json::to_string(&fingerprint).unwrap(),
                )
                .into_bytes();
                // Legacy bincode: the fourth enum variant's fixed u32 tag,
                // then the owned DefPath and optional observed fingerprint.
                let mut binary = 3u32.to_le_bytes().to_vec();
                binary.extend(bincode::serialize(&path).unwrap());
                binary.extend(bincode::serialize(&fingerprint).unwrap());
                let encode = || {
                    assert_eq!(serde_json::to_vec(&read).unwrap(), json);
                    assert_eq!(bincode::serialize(&read).unwrap(), binary);
                };
                let decode = || {
                    assert_eq!(
                        serde_json::from_slice::<hirsel_types::SharedRead>(&json).unwrap(),
                        read
                    );
                    assert_eq!(
                        bincode::deserialize::<hirsel_types::SharedRead>(&binary).unwrap(),
                        read
                    );
                };
                encode();
                decode();
                let mut numbering = Numbering::default();
                with_def_numbering(&mut numbering, encode);
                assert!(numbering.ids().is_empty());
                with_def_source(std::sync::Arc::new(NoDefinitions), decode);
                let DependencyRead::Signature { definition, .. } = read.as_ref() else {
                    unreachable!()
                };
                assert!(std::ptr::eq(definition.path(), DefId::intern(&path).path()));
            }
        }
    }
    #[test]
    fn dependency_order_is_irrelevant_but_text_and_signature_edits_invalidate() {
        let a = SignatureDependency {
            definition: hirsel_syntax::DefId::intern(&path("a")),
            fingerprint: ContentHash::of_bytes(b"a"),
        };
        let b = SignatureDependency {
            definition: hirsel_syntax::DefId::intern(&path("b")),
            fingerprint: ContentHash::of_bytes(b"b"),
        };
        let mut key = DefinitionKey {
            kind: QueryKind::Check,
            definition: path("main"),
            text_hash: ContentHash::of_bytes(b"body"),
            dependencies: vec![a.clone(), b.clone()],
            environment: QueryEnvironment {
                interface_version: 1,
                compiler_version: "0.1".into(),
                spec_hash: ContentHash::default(),
                package_manifest_hash: ContentHash::default(),
                builtin_signatures_hash: ContentHash::default(),
            },
        };
        let original = key.fingerprint().unwrap();
        key.dependencies = vec![b, a.clone(), a];
        assert_eq!(key.fingerprint().unwrap(), original);
        key.text_hash = ContentHash::of_bytes(b"new body");
        assert_ne!(key.fingerprint().unwrap(), original);
        key.text_hash = ContentHash::of_bytes(b"body");
        key.dependencies[0].fingerprint = ContentHash::of_bytes(b"new signature");
        assert_ne!(key.fingerprint().unwrap(), original);
        key.dependencies[2].fingerprint = ContentHash::of_bytes(b"inconsistent");
        assert!(key.fingerprint().is_err());
    }
}

mod stack;
pub use stack::{COMPILER_STACK_BYTES, with_compiler_stack};

pub mod runtime;

pub use queries::native_discovery::{
    NATIVE_DISCOVERY_KIND, NativeBodyContracts, NativeDiscoveryCache, NativeDiscoveryCounts,
    NativeDiscoveryProduct,
};
