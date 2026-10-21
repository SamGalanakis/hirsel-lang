//! Per-definition queries and phase adapters (LANG-6, PROG-5).

#[cfg(test)]
hirsel_guard::install!();

mod operations;
pub use operations::*;
mod native_audit;
pub use native_audit::{CleanupSite, SourceAudit};
mod native_input;
pub use native_input::*;
mod representation;
pub use representation::RepresentationRecord;
mod compiler;
mod comptime;
pub use compiler::Compiler;
mod materialized;
mod phases;
pub mod registry;
pub use phases::*;
mod diagnostics;
pub mod memo;
pub mod store;
pub use memo::{Durability, ReadKey, Reads, ReadsBuilder, Revision};
mod generic_graph;
pub use diagnostics::*;
pub use generic_graph::{InstanceEdge, InstanceGraph, InstanceKind};
mod summary;
pub use summary::{
    CallClass, DefinitionSummary, Frame, FrameKind, GenericParameterRef, GrowthEdge,
    ParameterGrowth, PublishedSummary, SummaryCall, growing_sites, summarize,
};
mod semantic;
mod snapshot;
pub use semantic::*;
pub use snapshot::{
    CitationSource, DeclarationIndex, Declarations, FrozenProvider, ModuleInputs, PackageViews,
    SignatureSnapshot, SnapshotSeed, View, ViewTable,
};
mod adapters;
mod facts;
pub use adapters::*;
pub mod image;
mod package;
pub use image::{ImageRecord, StdImage, StdImages, release_shared_caches, report_images};
mod policy;
pub use policy::{LoweringPolicy, Verify, free_sessions};
mod admission;
mod proof_export;
mod queries;
mod tools;
pub use admission::{
    admit_run, bind_run, callee_facts, entry_launch_kind, entry_roots, read_run_record,
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
        deps.sort_by(|a, b| a.definition.cmp(&b.definition));
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
            QueryKind::Lex => 4,
            QueryKind::Resolve => 5,
            QueryKind::Modes => 6,
            QueryKind::Parse => 0,
            QueryKind::Signature => 1,
            QueryKind::Check => 2,
            QueryKind::Lower => 3,
            QueryKind::Summary => 9,
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
}
/// Modes consume the complete typed artifact and refine its shared tables (OWN-1/8).
pub trait ModesPhase: Send + Sync {
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
    fn dependency_order_is_irrelevant_but_text_and_signature_edits_invalidate() {
        let a = SignatureDependency {
            definition: path("a"),
            fingerprint: ContentHash::of_bytes(b"a"),
        };
        let b = SignatureDependency {
            definition: path("b"),
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
