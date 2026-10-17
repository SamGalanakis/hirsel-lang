use crate::layered::Layered;
use crate::*;
mod dependencies;
mod lowering;
pub(crate) use lowering::Lowering;
mod package_image;
pub(crate) use package_image::{ImageInput, ImagePackage, PackageImage};
mod package_snapshot;
mod positions;
mod program_revision;
pub(crate) use program_revision::SemanticProgram;
mod signature_input;
pub(crate) use dependencies::ExecutionReads;
use hirsel_diag::Severity;
use hirsel_span::Span;
use hirsel_syntax::{ItemKind, ItemPath};
pub(crate) use package_snapshot::PackageSnapshot;
pub(crate) use positions::rebase_package_diagnostics;
pub use signature_input::SignatureInput;
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::Arc,
};

/// Signature normalization only reads the declared header and its contracts (LANG-6).
/// Revision-1 construction seam; the driver publishes its owned output as SignatureRef.
pub trait SignaturePhase {
    fn signature_items(
        &self,
        items: &[(Arc<SyntaxFile>, ItemId)],
        symbols: &Interner,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        let (file, item) = items.first().ok_or(QueryError::NotImplemented)?;
        self.signature(file.clone(), *item, symbols, provider)
    }
    fn signature(
        &self,
        file: Arc<SyntaxFile>,
        item: ItemId,
        symbols: &Interner,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError>;
}

/// Optional adapter slots support deliberately partial sessions in tooling tests.
/// Installed leaf adapters may also return unavailable or operational errors.
#[derive(Default)]
pub struct PhaseAdapters {
    pub lexer: Option<Rc<dyn LexerPhase>>,
    pub parser: Option<Rc<dyn ParserPhase>>,
    pub formatter: Option<Rc<dyn FormatterPhase>>,
    pub resolver: Option<Rc<dyn ResolverPhase>>,
    pub signature: Option<Rc<dyn SignaturePhase>>,
    pub checker: Option<Rc<dyn CheckerPhase>>,
    pub modes: Option<Rc<dyn ModesPhase>>,
    pub lowering: Option<Rc<dyn LoweringPhase>>,
    pub interpreter: Option<Box<dyn InterpreterPhase>>,
    pub compiler_state: Option<Rc<std::cell::RefCell<CompilerState>>>,
    /// The front phases a complete compiler was built from (`PhaseAdapters::compiler`).
    pub phases: Option<Rc<dyn Phases>>,
}

/// Reads survive errors and missing phases, rather than being lost in QueryError.
#[derive(Clone, Debug, Default)]
pub struct DependencyReads {
    pub operational_failure: Option<hirsel_types::OperationalFailure>,
    pub signatures: Vec<SignatureDependency>,
    pub failed_signatures: Vec<DefPath>,
    pub failed_signature_keys: Vec<(DefPath, Option<ContentHash>)>,
    pub lookups: Vec<(DefPath, bool)>,
    pub manifest_lookups: Vec<hirsel_types::DependencyRead>,
    pub resolver: Vec<hirsel_resolve::DependencyRead>,
}
#[derive(Clone)]
struct Cached<T> {
    id: u64,
    key: DefinitionKey,
    reads: DependencyReads,
    result: Result<Arc<QueryOutput<T>>, QueryError>,
    lookups: dependencies::LookupInputs,
    executed: ExecutionReads,
    /// Body-only receipts, separate from the public query schemas and fingerprints.
    representations: BTreeMap<DefPath, ContentHash>,
    /// A signature's fingerprint coverage: the adapter's own dependency list,
    /// before the query's reads join it in the output (PS-9).
    covered: Vec<SignatureDependency>,
}

impl<T: Clone> Cached<T> {
    fn output(&self) -> Result<QueryOutput<T>, QueryError> {
        self.result
            .as_ref()
            .map(|output| (**output).clone())
            .map_err(Clone::clone)
    }
}
#[derive(Default)]
struct ValidationWalk {
    // Completed immutable nodes, scoped to a query with unchanged source inputs.
    // A freshly computed entry has a new ID and still gets its first replay.
    visited: BTreeSet<u64>,
}

/// A file's parse result under its package's query environment.
pub(crate) type ParsedFile = (
    FileContext,
    QueryEnvironment,
    Result<QueryOutput<SyntaxFile>, QueryError>,
);
#[derive(Clone)]
struct ItemSnapshot {
    file: Arc<SyntaxFile>,
    item: ItemId,
}

/// A single coherent session. Reused owners keep their arena IDs while their
/// origins and cached artifact sites are re-spanned to the current source revision.
pub struct QueryEngine {
    pub comptime_limits: hirsel_interp::Limits,
    pub last_comptime_limit: Option<hirsel_tir::ToolingLimit>,
    pub(crate) comptime_stack: Vec<(DefPath, hirsel_span::Span)>,
    pub sources: SourceMap,
    pub symbols: Interner,
    pub environment: QueryEnvironment,
    pub adapters: PhaseAdapters,
    items: Layered<DefPath, Vec<ItemSnapshot>>,
    parsed: Layered<FileId, ParsedFile>,
    registry: crate::registry::BuiltinRegistry,
    builtins: BTreeMap<DefPath, hirsel_types::SignatureRef>,
    syntax_keys: Layered<(DefPath, QueryKind), DefinitionKey>,
    signatures: Layered<DefPath, Arc<Cached<hirsel_types::SignatureRef>>>,
    resolutions: Layered<DefPath, Arc<Cached<Resolution>>>,
    checked: Layered<DefPath, Arc<Cached<TypedBody>>>,
    modes: Layered<DefPath, Arc<Cached<AnalyzedBody>>>,
    lowered: Layered<DefPath, Arc<Cached<hirsel_tir::DefinitionRef>>>,
    manifest_identity_hash: ContentHash,
    /// Dependency packages key on their own closure's manifests, not the whole
    /// graph's, so their queries are shared across programs (package images).
    pub(crate) package_manifests: BTreeMap<hirsel_syntax::PackageId, ContentHash>,
    pub(crate) execution_stack: Vec<ExecutionReads>,
    active: BTreeSet<(DefPath, QueryKind)>,
    validation_walk: Option<ValidationWalk>,
    cache_sequence: u64,
    source_revision: u64,
    package_proofs: BTreeMap<hirsel_syntax::PackageId, package_snapshot::PackageProof>,
    package_publications: BTreeMap<hirsel_syntax::PackageId, package_snapshot::PackagePublication>,
    pub(crate) generic_verdict: Option<crate::generic_graph::Verdict>,
    signature_input: BTreeMap<hirsel_syntax::PackageId, signature_input::ProgramInput>,
    signature_stack: Vec<DefPath>,
    recomputes: BTreeMap<(DefPath, QueryKind), usize>,
    reads: Layered<(DefPath, QueryKind), DependencyReads>,
    pub unavailable: Option<QueryKind>,
    pub last_phase: Option<QueryKind>,
}
impl QueryEngine {
    /// Every indexed source definition.
    pub(crate) fn item_paths(&self) -> Vec<DefPath> {
        self.items.keys().cloned().collect()
    }
    /// The source definitions a native export selects: the semantic closure
    /// (`semantic_program`, the closure check-time lowering and the program
    /// revision use) of every definition outside the dependency packages. A
    /// dependency definition the program never reaches is not compiled. An
    /// engine without dependency packages selects every definition.
    pub(crate) fn native_paths(&mut self) -> Result<BTreeSet<DefPath>, QueryError> {
        let roots: Vec<_> = self
            .items
            .keys()
            .filter(|path| !self.package_manifests.contains_key(&path.0.package))
            .cloned()
            .collect();
        let mut selected = self.semantic_program(roots)?.definitions;
        selected.retain(|path| self.items.contains_key(path));
        Ok(selected)
    }
    pub(crate) fn native_builtin_signatures(
        &self,
    ) -> BTreeMap<DefPath, hirsel_types::SignatureRef> {
        self.registry
            .signatures()
            .map(|s| (s.definition.clone(), s.clone()))
            .chain(self.builtins.iter().map(|(p, s)| (p.clone(), s.clone())))
            .collect()
    }
    pub(crate) fn registry_dictionary_value(
        &self,
        path: &DefPath,
    ) -> Result<Option<hirsel_tir::Materialized>, QueryError> {
        if self.definition_items(path).is_ok() {
            return Ok(None);
        }
        self.registry.dictionary_value(path)
    }
    pub(crate) fn prepare_materialization_environment(&mut self) {
        if let Some(state) = &self.adapters.compiler_state {
            let mut state = state.borrow_mut();
            if state.materialization_environment.as_ref() != Some(&self.environment)
                || state.materialization_limits != Some(self.comptime_limits)
            {
                state.constant_values.clear();
                self.lowered.clear();
                state.materialization_environment = Some(self.environment.clone());
                state.materialization_limits = Some(self.comptime_limits);
            }
        }
    }

    pub(crate) fn validate_comptime(
        &mut self,
        definition: &hirsel_tir::Definition,
        artifact: Signature,
    ) -> Result<(), QueryError> {
        let signatures = self.validated_signature_input(&definition.path.0.package);
        let report = signatures.validate_with_signature(definition, &artifact);
        if let Some(error) = hirsel_validate::tooling_limit_error(&report) {
            return Err(error);
        }
        if report.violations.is_empty() {
            Ok(())
        } else {
            Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "compile-time TIR validation".into(),
                message: format!("{:?}", report.violations),
            }))
        }
    }
    pub fn new(mut environment: QueryEnvironment, adapters: PhaseAdapters) -> Self {
        let registry = crate::registry::BuiltinRegistry::new();
        environment.builtin_signatures_hash = registry.revision();
        Self {
            comptime_limits: hirsel_interp::Limits::default(),
            last_comptime_limit: None,
            comptime_stack: vec![],
            sources: SourceMap::default(),
            symbols: Interner::default(),
            environment,
            adapters,
            items: Layered::default(),
            parsed: Layered::default(),
            registry,
            builtins: BTreeMap::new(),
            syntax_keys: Layered::default(),
            signatures: Layered::default(),
            resolutions: Layered::default(),
            checked: Layered::default(),
            modes: Layered::default(),
            lowered: Layered::default(),
            manifest_identity_hash: ContentHash::default(),
            package_manifests: BTreeMap::new(),
            execution_stack: vec![],
            active: BTreeSet::new(),
            validation_walk: None,
            cache_sequence: 0,
            source_revision: 0,
            package_proofs: BTreeMap::new(),
            package_publications: BTreeMap::new(),
            generic_verdict: None,
            signature_input: BTreeMap::new(),
            signature_stack: vec![],
            recomputes: BTreeMap::new(),
            reads: Layered::default(),
            unavailable: None,
            last_phase: None,
        }
    }
    /// The environment a package's queries and parses key on.
    pub(crate) fn package_environment(
        &self,
        package: &hirsel_syntax::PackageId,
    ) -> QueryEnvironment {
        let mut environment = self.environment.clone();
        if let Some(manifest) = self.package_manifests.get(package) {
            environment.package_manifest_hash = *manifest;
        }
        environment
    }
    pub fn recompute_count(&self, definition: &DefPath, kind: QueryKind) -> usize {
        self.recomputes
            .get(&(definition.clone(), kind))
            .copied()
            .unwrap_or(0)
    }
    pub fn dependency_reads(
        &self,
        definition: &DefPath,
        kind: QueryKind,
    ) -> Option<&DependencyReads> {
        self.reads.get(&(definition.clone(), kind))
    }
    fn missing<T>(&mut self, kind: QueryKind) -> Result<T, QueryError> {
        self.unavailable = Some(kind);
        Err(QueryError::NotImplemented)
    }
    fn snapshot(&self, definition: &DefPath) -> Result<ItemSnapshot, QueryError> {
        self.items
            .get(definition)
            .and_then(|items| items.first())
            .cloned()
            .ok_or(QueryError::NotImplemented)
    }
    fn key(&self, definition: &DefPath, kind: QueryKind) -> Result<DefinitionKey, QueryError> {
        let snapshot = self.snapshot(definition)?;
        let item = &snapshot.file.tree.items[snapshot.item.0 as usize];
        let text_hash = if kind == QueryKind::Signature {
            let hashes: Vec<_> = self
                .items
                .get(definition)
                .unwrap()
                .iter()
                .map(|snapshot| {
                    let item = &snapshot.file.tree.items[snapshot.item.0 as usize];
                    let text = self.sources.file(snapshot.file.file).unwrap().text();
                    ContentHash::of_bytes(
                        text[header_span(&snapshot.file, item).start as usize
                            ..header_span(&snapshot.file, item).end as usize]
                            .as_bytes(),
                    )
                })
                .collect();
            ContentHash::of_parts(
                "hirsel.signature.inputs.v1",
                hashes.iter().map(|h| h.0.as_slice()),
            )
        } else {
            item.text_hash
        };
        Ok(DefinitionKey {
            kind,
            definition: definition.clone(),
            text_hash,
            dependencies: vec![],
            environment: self.package_environment(&definition.0.package),
        })
    }
    fn signature_query(
        &mut self,
        definition: &DefPath,
    ) -> Result<Arc<QueryOutput<hirsel_types::SignatureRef>>, QueryError> {
        if !self.items.contains_key(definition) {
            if let Some(signature) = self.registry.get(definition) {
                return Ok(Arc::new(output(signature)));
            }
            if let Some(signature) = self.builtins.get(definition) {
                return Ok(Arc::new(output(signature.clone())));
            }
        }
        if let Some(cached) = self.signatures.get(definition).cloned() {
            if self
                .active
                .contains(&(definition.clone(), QueryKind::Signature))
            {
                return self.reentrant(definition, QueryKind::Signature);
            }
            if self
                .validation_walk
                .as_ref()
                .unwrap()
                .visited
                .contains(&cached.id)
            {
                return cached.result.clone();
            }
            let key = self.key(definition, QueryKind::Signature)?;
            self.active
                .insert((definition.clone(), QueryKind::Signature));
            let valid = self.valid(&cached, &key);
            self.active
                .remove(&(definition.clone(), QueryKind::Signature));
            if valid {
                return cached.result.clone();
            }
        }
        let key = self.key(definition, QueryKind::Signature)?;
        self.start(definition, QueryKind::Signature)?;
        let symbols = self.symbols.clone();
        let candidates = self.definition_items(definition)?;
        let adapter = self.adapters.signature.clone();
        let mut recorder = Recorder {
            owner: definition.clone(),
            private_shapes: false,
            representations: BTreeMap::new(),
            engine: self,
            reads: DependencyReads::default(),
        };
        recorder.engine.signature_stack.push(definition.clone());
        let result = match adapter {
            Some(adapter) => adapter.signature_items(&candidates, &symbols, &mut recorder),
            None => recorder.engine.missing(QueryKind::Signature),
        };
        recorder.engine.signature_stack.pop();
        let representations = recorder.representations;
        let reads = recorder.reads;
        let covered = result
            .as_ref()
            .map(|output| output.dependencies.clone())
            .unwrap_or_default();
        let mut cached = self.finish(key, reads, result.map(shared_output), representations);
        Arc::make_mut(&mut cached).covered = covered;
        if !cached.result.as_ref().is_err_and(|e| !e.cacheable()) {
            self.signatures.insert(definition.clone(), cached.clone());
        }
        cached.result.clone()
    }
    fn with_validation_walk<T>(&mut self, query: impl FnOnce(&mut Self) -> T) -> T {
        let root = self.validation_walk.is_none();
        if root {
            self.validation_walk = Some(ValidationWalk::default());
        }
        let result = query(self);
        if root {
            self.validation_walk = None;
        }
        result
    }
    fn valid<T>(&mut self, cached: &Cached<T>, key: &DefinitionKey) -> bool {
        self.with_validation_walk(|engine| engine.valid_node(cached, key))
    }
    fn valid_node<T>(&mut self, cached: &Cached<T>, key: &DefinitionKey) -> bool {
        if cached
            .result
            .as_ref()
            .is_err_and(|error| !error.cacheable())
        {
            return false;
        }
        if cached.key.kind != key.kind
            || cached.key.definition != key.definition
            || cached.key.text_hash != key.text_hash
            || cached.key.environment != key.environment
        {
            return false;
        }
        if self
            .package_proofs
            .get(&key.definition.0.package)
            .is_some_and(|proof| proof.matches(self))
        {
            return true;
        }
        if self
            .validation_walk
            .as_ref()
            .unwrap()
            .visited
            .contains(&cached.id)
        {
            return true;
        }
        hirsel_intern::work::count(hirsel_intern::work::Counter::CacheValidationRows);
        if !self.lookup_reads_valid(&cached.reads, &cached.lookups, &key.definition)
            || !self.executions_valid(&cached.executed)
            || cached
                .representations
                .iter()
                .any(|(path, hash)| self.implementation_hash(path) != Some(*hash))
        {
            return false;
        }
        for dep in &cached.reads.signatures {
            if !self
                .signature(&dep.definition)
                .is_ok_and(|s| s.fingerprint == dep.fingerprint)
            {
                return false;
            }
        }
        for (path, previous) in &cached.reads.failed_signature_keys {
            if self.signature(path).is_ok() || self.failed_signature_key(path) != *previous {
                return false;
            }
        }
        self.validation_walk
            .as_mut()
            .unwrap()
            .visited
            .insert(cached.id);
        true
    }
    fn failed_signature_key(&self, path: &DefPath) -> Option<ContentHash> {
        self.signatures
            .get(path)
            .and_then(|cached| cached.key.fingerprint().ok())
    }
    fn reentrant<T>(&mut self, definition: &DefPath, kind: QueryKind) -> Result<T, QueryError> {
        if kind == QueryKind::Signature
            && self
                .signature_stack
                .last()
                .is_some_and(|owner| owner != definition)
        {
            let snapshot = self.snapshot(definition)?;
            if let ItemKind::Type {
                body: hirsel_syntax::TypeBody::Alias(target),
                ..
            } = &snapshot.file.tree.items[snapshot.item.0 as usize].kind
            {
                return Err(QueryError::Diagnostics(vec![Diagnostic::error(
                    const { hirsel_diag::RuleId::of("LANG-7") },
                    snapshot.file.tree.types[target.0 as usize].origin.span,
                    "cyclic type aliases have no representation",
                )]));
            }
        }
        // Direct self-reference remains the signature/layout phase's obligation;
        // cache receipt replay also retains its existing deterministic rejection.
        self.missing(kind)
    }
    fn start(&mut self, definition: &DefPath, kind: QueryKind) -> Result<(), QueryError> {
        self.last_phase = Some(kind);
        if !self.active.insert((definition.clone(), kind)) {
            return self.reentrant(definition, kind);
        }
        *self
            .recomputes
            .entry((definition.clone(), kind))
            .or_default() += 1;
        Ok(())
    }
    fn finish<T>(
        &mut self,
        mut key: DefinitionKey,
        mut reads: DependencyReads,
        mut result: Result<QueryOutput<T>, QueryError>,
        representations: BTreeMap<DefPath, ContentHash>,
    ) -> Arc<Cached<T>> {
        if let Ok(output) = &mut result {
            reads.signatures.extend(output.dependencies.clone());
            output.dependencies = reads.signatures.clone();
            if has_errors(&output.diagnostics) {
                result = Err(QueryError::Diagnostics(output.diagnostics.clone()));
            }
        }
        if let Some(error) = &reads.operational_failure {
            result = Err(QueryError::Operational(error.clone()));
        }
        key.dependencies = reads.signatures.clone();
        if key.fingerprint().is_err() {
            result = Err(QueryError::NotImplemented);
        }
        if let Some(state) = &self.adapters.compiler_state {
            let state = state.borrow();
            let native = match key.kind {
                QueryKind::Resolve => state.resolver_outputs.get(&key.definition),
                QueryKind::Signature => state.signature_resolutions.get(&key.definition),
                _ => None,
            };
            if let Some(native) = native {
                reads.resolver = native.reads.clone();
            }
        }
        self.active.remove(&(key.definition.clone(), key.kind));
        if result.is_err() {
            self.last_phase = Some(key.kind);
        }
        self.reads
            .insert((key.definition.clone(), key.kind), reads.clone());
        let lookups = self.lookup_inputs(&reads);
        self.cache_sequence += 1;
        Arc::new(Cached {
            id: self.cache_sequence,
            key,
            reads,
            result: result.map(Arc::new),
            lookups,
            executed: ExecutionReads::default(),
            representations,
            covered: vec![],
        })
    }
    /// Parsing a new snapshot updates definitions in that manifest module. Unchanged items
    /// retain their old owning AST, so body edits in siblings don't invalidate their queries.
    fn index(&mut self, file: &SyntaxFile, _context: &FileContext) -> Result<(), QueryError> {
        self.source_revision += 1;
        let source_path = &self.sources.file(file.file).unwrap().path;
        let from_source =
            |old: &ItemSnapshot| &self.sources.file(old.file.file).unwrap().path == source_path;
        // Only definitions this source owned or now owns can change; every
        // other entry, including a shared image's, is left untouched.
        let owned: Vec<DefPath> = self
            .items
            .iter()
            .filter(|(_, candidates)| candidates.iter().any(from_source))
            .map(|(path, _)| path.clone())
            .collect();
        let mut previous: Vec<_> = owned
            .iter()
            .flat_map(|path| &self.items[path])
            .filter(|old| from_source(old))
            .cloned()
            .collect();
        // Signature queries can survive a body/trivia edit as well as whole-item
        // reuse. Match their unchanged header independently before retiring owners.
        let mut movements = vec![];
        let mut matched = BTreeSet::new();
        for id in &file.roots {
            let item = &file.tree.items[id.0 as usize];
            let candidate = previous
                .iter()
                .enumerate()
                .filter(|(i, old)| {
                    !matched.contains(i)
                        && old.file.tree.items[old.item.0 as usize].definition == item.definition
                })
                .find(|(_, old)| {
                    old.file.tree.items[old.item.0 as usize].text_hash == item.text_hash
                })
                .or_else(|| {
                    previous.iter().enumerate().find(|(i, old)| {
                        if matched.contains(i) {
                            return false;
                        }
                        let before = &old.file.tree.items[old.item.0 as usize];
                        before.definition == item.definition
                            && self.sources.slice(header_span(&old.file, before))
                                == self.sources.slice(header_span(file, item))
                    })
                });
            if let Some((i, old)) = candidate {
                matched.insert(i);
                let before = &old.file.tree.items[old.item.0 as usize];
                let span = if before.text_hash == item.text_hash {
                    let start = old
                        .file
                        .tokens
                        .get(before.owned_tokens.start as usize)
                        .map_or(before.origin.span.start, |t| t.span.start);
                    Span {
                        start,
                        ..before.origin.span
                    }
                } else {
                    header_span(&old.file, before)
                };
                movements.push(positions::Movement {
                    old: span,
                    new_file: file.file,
                    delta: i64::from(item.origin.span.start) - i64::from(before.origin.span.start),
                    whole: (before.origin.span, item.origin.span),
                });
            }
        }
        for path in &owned {
            let sources = &self.sources;
            let candidates = self.items.get_mut(path).unwrap();
            candidates.retain(|old| &sources.file(old.file.file).unwrap().path != source_path);
            if candidates.is_empty() {
                self.items.remove(path);
            }
        }
        let mut affected: BTreeSet<DefPath> = owned.into_iter().collect();
        let shared_file = Arc::new(file.clone());
        for id in &file.roots {
            let item = &file.tree.items[id.0 as usize];
            let unchanged = previous.iter().position(|old| {
                let old_item = &old.file.tree.items[old.item.0 as usize];
                old_item.definition == item.definition && old_item.text_hash == item.text_hash
            });
            let snapshot = unchanged
                .map(|index| previous.remove(index))
                .unwrap_or_else(|| ItemSnapshot {
                    file: shared_file.clone(),
                    item: *id,
                });
            // Duplicate paths keep one semantic identity and multiple addressable items.
            // Task E diagnoses overloading as LANG-7 at the later definition's name.
            if let Some((helper_file, helper_item)) =
                crate::facts::snapshot(&snapshot.file, snapshot.item, &mut self.symbols)
            {
                let helper = &helper_file.tree.items[helper_item.0 as usize];
                affected.insert(helper.definition.clone());
                self.items
                    .entry_or_default(helper.definition.clone())
                    .push(ItemSnapshot {
                        file: helper_file,
                        item: helper_item,
                    });
            }
            affected.insert(item.definition.clone());
            self.items
                .entry_or_default(item.definition.clone())
                .push(snapshot);
        }
        // Retire changed inputs before moving surviving sites. Otherwise a deleted
        // or renamed definition can later revive an old key whose sites skipped
        // intervening source revisions. Unchanged headers survive body edits.
        let body_hashes: BTreeMap<_, _> = affected
            .iter()
            .filter(|path| self.items.contains_key(*path))
            .map(|path| {
                (
                    path.clone(),
                    self.key(path, QueryKind::Check).unwrap().text_hash,
                )
            })
            .collect();
        let header_hashes: BTreeMap<_, _> = affected
            .iter()
            .filter(|path| self.items.contains_key(*path))
            .map(|path| {
                (
                    path.clone(),
                    self.key(path, QueryKind::Signature).unwrap().text_hash,
                )
            })
            .collect();
        macro_rules! retire {
            ($field:ident, $hashes:ident) => {
                for path in &affected {
                    if self
                        .$field
                        .get(path)
                        .is_some_and(|c| $hashes.get(path) != Some(&c.key.text_hash))
                    {
                        self.$field.remove(path);
                    }
                }
            };
        }
        retire!(signatures, header_hashes);
        retire!(resolutions, body_hashes);
        retire!(checked, body_hashes);
        retire!(modes, body_hashes);
        retire!(lowered, body_hashes);
        if let Some(state) = &self.adapters.compiler_state {
            let mut state = state.borrow_mut();
            state.constant_values.retain(|_, value| {
                value.executed.bodies.iter().all(|(path, hash)| {
                    !affected.contains(path) || body_hashes.get(path) == Some(hash)
                })
            });
            for path in &affected {
                if !self.resolutions.contains_key(path) {
                    state.resolver_outputs.remove(path);
                }
                if !self.signatures.contains_key(path) {
                    state.signature_resolutions.remove(path);
                }
            }
        }
        self.rebase_positions(&movements);
        self.publish_declarations();
        Ok(())
    }
    /// Publish the indexed declarations: manifest identities and the
    /// resolver's module declarations.
    pub(crate) fn publish_declarations(&mut self) {
        let hashes: Vec<_> = self
            .items
            .iter()
            .flat_map(|(path, candidates)| {
                let item = &candidates[0].file.tree.items[candidates[0].item.0 as usize];
                let public = item.visibility != hirsel_syntax::Visibility::Private;
                let kind = match &item.kind {
                    ItemKind::Function { .. } => 0,
                    ItemKind::Const { .. } => 1,
                    ItemKind::Type {
                        body: hirsel_syntax::TypeBody::Alias(_),
                        ..
                    } => 2,
                    ItemKind::Type { .. } => 3,
                };
                [
                    path.fingerprint(),
                    ContentHash::of_bytes(&[u8::from(public), kind]),
                ]
            })
            .collect();
        self.manifest_identity_hash = ContentHash::of_parts(
            "hirsel.manifest.identities.v1",
            hashes.iter().map(|h| h.0.as_slice()),
        );
        if let Some(state) = &self.adapters.compiler_state {
            let mut state = state.borrow_mut();
            state.set_declarations(
                self.items
                    .values()
                    .flatten()
                    .map(|item| (item.file.clone(), item.item)),
            );
            state.manifest_revision = ContentHash::of_parts(
                "hirsel.manifest.v1",
                [
                    self.environment.package_manifest_hash.0.as_slice(),
                    self.manifest_identity_hash.0.as_slice(),
                ],
            );
        }
    }
    /// All source items sharing a semantic path, in index order. Ordinals stay local;
    /// neither duplicates nor file positions are ever appended to DefPath identity.
    pub fn definition_items(
        &self,
        definition: &DefPath,
    ) -> Result<Vec<(Arc<SyntaxFile>, ItemId)>, QueryError> {
        self.items
            .get(definition)
            .map(|items| {
                items
                    .iter()
                    .map(|item| (item.file.clone(), item.item))
                    .collect()
            })
            .ok_or(QueryError::NotImplemented)
    }
    /// The item remains indexed within the returned immutable owning file; clients
    /// must use this ItemId with this file, never another revision's AST.
    pub fn parse_definition(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<(Arc<SyntaxFile>, ItemId)>, QueryError> {
        self.syntax_view(definition, QueryKind::Parse)?;
        let snapshot = self.snapshot(definition)?;
        Ok(output((snapshot.file, snapshot.item)))
    }
    fn syntax_view(&mut self, definition: &DefPath, kind: QueryKind) -> Result<(), QueryError> {
        let key = self.key(definition, kind)?;
        let identity = (definition.clone(), kind);
        if self.syntax_keys.get(&identity) != Some(&key) {
            *self.recomputes.entry(identity.clone()).or_default() += 1;
            self.syntax_keys.insert(identity, key);
        }
        Ok(())
    }
    pub fn resolve_definition(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<Resolution>, QueryError> {
        self.resolve_query(definition)
            .map(|output| (*output).clone())
    }
    /// The lexing and parsing views resolution reads, without copying them.
    fn syntax_views(&mut self, definition: &DefPath) -> Result<(), QueryError> {
        self.syntax_view(definition, QueryKind::Lex)?;
        let snapshot = self.snapshot(definition)?;
        let range = snapshot.file.tree.items[snapshot.item.0 as usize].owned_tokens;
        if snapshot
            .file
            .tokens
            .get(range.start as usize..range.end as usize)
            .is_none()
        {
            return Err(QueryError::NotImplemented);
        }
        self.syntax_view(definition, QueryKind::Parse)
    }
    /// The signature's shared output: cache hits never copy it.
    /// The dependency list `definition`'s fingerprint covers (PS-9): a
    /// declaration's normalized dependencies, a fact helper's owner, nothing
    /// for a registry row. The query output's dependencies are its reads,
    /// which also name what only resolution or a private representation read.
    pub fn signature_coverage(
        &mut self,
        definition: &DefPath,
    ) -> Result<Vec<SignatureDependency>, QueryError> {
        let output = self.signature_output(definition)?;
        Ok(match self.signatures.get(definition) {
            Some(cached) if self.items.contains_key(definition) => cached.covered.clone(),
            _ => output.dependencies.clone(),
        })
    }
    fn signature_output(
        &mut self,
        definition: &DefPath,
    ) -> Result<Arc<QueryOutput<hirsel_types::SignatureRef>>, QueryError> {
        self.with_validation_walk(|engine| engine.signature_query(definition))
    }
    fn resolve_query(
        &mut self,
        definition: &DefPath,
    ) -> Result<Arc<QueryOutput<Resolution>>, QueryError> {
        self.syntax_views(definition)?;
        let key = self.key(definition, QueryKind::Resolve)?;
        if let Some(cached) = self.resolutions.get(definition).cloned() {
            if self.valid(&cached, &key) {
                return cached.result.clone();
            }
        }
        self.start(definition, QueryKind::Resolve)?;
        let snapshot = self.snapshot(definition)?;
        let symbols = self.symbols.clone();
        let adapter = self.adapters.resolver.clone();
        let mut recorder = Recorder {
            owner: definition.clone(),
            private_shapes: true,
            representations: BTreeMap::new(),
            engine: self,
            reads: DependencyReads::default(),
        };
        let result = match adapter {
            Some(adapter) => adapter.resolve(
                snapshot.file.clone(),
                snapshot.item,
                &symbols,
                &mut recorder,
            ),
            None => recorder.engine.missing(QueryKind::Resolve),
        };
        let representations = recorder.representations;
        let reads = recorder.reads;
        let cached = self.finish(key, reads, result, representations);
        if !cached.result.as_ref().is_err_and(|e| !e.cacheable()) {
            self.resolutions.insert(definition.clone(), cached.clone());
        }
        cached.result.clone()
    }
    pub fn mode_definition(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<AnalyzedBody>, QueryError> {
        self.mode_query(definition).map(|output| (*output).clone())
    }
    pub(crate) fn mode_query(
        &mut self,
        definition: &DefPath,
    ) -> Result<Arc<QueryOutput<AnalyzedBody>>, QueryError> {
        let checked = self.check_query(definition)?;
        let signature = self.signature_output(definition)?;
        let key = self.key(definition, QueryKind::Modes)?;
        let cached = self.modes.get(definition).cloned();
        let body = if let Some(cached) = cached.filter(|c| self.valid(c, &key)) {
            cached.result.clone()?
        } else {
            self.start(definition, QueryKind::Modes)?;
            let adapter = self.adapters.modes.clone();
            let reads = self
                .checked
                .get(definition)
                .map(|c| c.reads.clone())
                .unwrap_or_else(|| DependencyReads {
                    signatures: checked.dependencies.clone(),
                    ..Default::default()
                });
            let mut recorder = Recorder {
                owner: definition.clone(),
                private_shapes: true,
                representations: self
                    .checked
                    .get(definition)
                    .map(|c| c.representations.clone())
                    .unwrap_or_default(),
                engine: self,
                reads,
            };
            let result = match adapter {
                Some(adapter) => {
                    adapter.check_modes(checked.value.clone(), &signature.value, &mut recorder)
                }
                None => recorder.engine.missing(QueryKind::Modes),
            };
            let representations = recorder.representations;
            let reads = recorder.reads;
            let cached = self.finish(key, reads, result, representations);
            if !cached.result.as_ref().is_err_and(|e| !e.cacheable()) {
                self.modes.insert(definition.clone(), cached.clone());
            }
            cached.result.clone()?
        };
        // Compile-time users also depend on private shapes used by a body,
        // including when its public signature and source text are unchanged.
        let representations: Vec<_> = self
            .modes
            .get(definition)
            .map(|c| c.representations.keys().cloned().collect())
            .unwrap_or_default();
        for path in representations {
            self.read_implementation(&path);
        }
        Ok(body)
    }
    fn check_query(
        &mut self,
        definition: &DefPath,
    ) -> Result<Arc<QueryOutput<TypedBody>>, QueryError> {
        let resolution = self.resolve_query(definition)?;
        let signature = self.signature_output(definition)?;
        let key = self.key(definition, QueryKind::Check)?;
        if let Some(cached) = self.checked.get(definition).cloned() {
            if self.valid(&cached, &key) {
                return cached.result.clone();
            }
        }
        let mut initial = self
            .resolutions
            .get(definition)
            .map(|c| c.reads.clone())
            .unwrap_or_default();
        initial.signatures.extend(resolution.dependencies.clone());
        initial.signatures.extend(signature.dependencies.clone());
        initial.signatures.push(SignatureDependency {
            definition: definition.clone(),
            fingerprint: signature.value.fingerprint,
        });
        self.start(definition, QueryKind::Check)?;
        let snapshot = self.snapshot(definition)?;
        let adapter = self.adapters.checker.clone();
        let mut recorder = Recorder {
            owner: definition.clone(),
            private_shapes: true,
            representations: self
                .resolutions
                .get(definition)
                .map(|c| c.representations.clone())
                .unwrap_or_default(),
            engine: self,
            reads: initial,
        };
        let result = match adapter {
            Some(adapter) => adapter.check(
                snapshot.file.clone(),
                snapshot.item,
                resolution.value.clone(),
                &signature.value,
                &mut recorder,
            ),
            None => recorder.engine.missing(QueryKind::Check),
        };
        let representations = recorder.representations;
        let reads = recorder.reads;
        let cached = self.finish(key, reads, result, representations);
        if !cached.result.as_ref().is_err_and(|e| !e.cacheable()) {
            self.checked.insert(definition.clone(), cached.clone());
        }
        cached.result.clone()
    }
    /// GEN-5 owns these body snapshots independently of per-definition cache keys.
    pub(crate) fn instantiation_bodies(
        &mut self,
    ) -> Result<Vec<(DefPath, Arc<QueryOutput<TypedBody>>)>, QueryError> {
        let mut bodies = vec![];
        for path in self.items.keys().cloned().collect::<Vec<_>>() {
            let checked = self.check_query(&path)?;
            bodies.push((path, checked));
        }
        Ok(bodies)
    }
    pub fn diagnostic_phase(&self, diagnostic: &Diagnostic) -> Option<String> {
        if let Some(state) = &self.adapters.compiler_state {
            if let Some((_, phase)) = state
                .borrow()
                .mode_diagnostics
                .iter()
                .find(|(d, _)| d.rule == diagnostic.rule && d.primary == diagnostic.primary)
            {
                return Some(phase.clone());
            }
        }
        fn contains<T>(cached: &Cached<T>, diagnostic: &Diagnostic) -> bool {
            let diagnostics = match &cached.result {
                Ok(o) => &o.diagnostics,
                Err(QueryError::Diagnostics(ds)) => ds,
                _ => return false,
            };
            diagnostics
                .iter()
                .any(|d| d.rule == diagnostic.rule && d.primary == diagnostic.primary)
        }
        if self.resolutions.values().any(|c| contains(c, diagnostic)) {
            return Some("name resolution".into());
        }
        if self.signatures.values().any(|c| contains(c, diagnostic)) {
            return Some("signature normalization".into());
        }
        if self.checked.values().any(|c| contains(c, diagnostic)) {
            return Some("body checking".into());
        }
        self.last_phase.map(|p| p.phase_name().into())
    }
    pub fn runtime_types(&mut self) -> Result<hirsel_interp::aggregate::RuntimeTypes, QueryError> {
        self.runtime_types_with_finalizers(true)
    }
    /// Runtime metadata plus the declared facts checked execution enforces:
    /// `@noalloc` definitions, library `in ..` results and `arena.with`.
    pub fn checked_runtime_types(
        &mut self,
    ) -> Result<hirsel_interp::aggregate::RuntimeTypes, QueryError> {
        let mut runtime = self.runtime_types()?;
        let mut checked = hirsel_interp::checked::CheckedRuntime {
            arena_with: Some(hirsel_syntax::DefPath::core("arena.with")),
            ..Default::default()
        };
        let library: Vec<_> = self.registry.signatures().cloned().collect();
        let mut signatures: Vec<_> = library.iter().cloned().collect();
        for path in self.items.keys().cloned().collect::<Vec<_>>() {
            signatures.push(self.item_signature(&path)?.value);
        }
        for signature in signatures {
            if let hirsel_types::SignatureKind::Type(t) = &signature.kind {
                if t.loop_source.is_some() {
                    checked
                        .operations
                        .insert(signature.definition.clone(), signature.clone());
                }
                if t.destructor_allocation == hirsel_types::Allocation::NoAlloc {
                    checked.noalloc_drop.insert(signature.definition.clone());
                    checked.noalloc.insert(signature.definition.clone());
                }
            }
            let hirsel_types::SignatureKind::Function(function) = &signature.kind else {
                continue;
            };
            if function.allocation == hirsel_types::Allocation::NoAlloc {
                checked.noalloc.insert(signature.definition.clone());
            }
            checked
                .operations
                .insert(signature.definition.clone(), signature);
        }
        if let Some(forget) = &runtime.forget {
            checked.noalloc.insert(forget.clone());
        }
        runtime.checked = Some(checked);
        Ok(runtime)
    }
    pub(crate) fn comptime_runtime_types(
        &mut self,
    ) -> Result<hirsel_interp::aggregate::RuntimeTypes, QueryError> {
        self.runtime_types_with_finalizers(false)
    }
    fn runtime_types_with_finalizers(
        &mut self,
        eager: bool,
    ) -> Result<hirsel_interp::aggregate::RuntimeTypes, QueryError> {
        let mut runtime = hirsel_interp::aggregate::RuntimeTypes {
            trace_mode: hirsel_interp::TraceMode::Effects,
            forget: Some(hirsel_syntax::DefPath::core(
                hirsel_interp::library::FORGET_OPERATION,
            )),
            ..Default::default()
        };
        let mut signatures: Vec<_> = self.registry.signatures().cloned().collect();
        for path in self.items.keys().cloned().collect::<Vec<_>>() {
            let public = self.item_signature(&path)?.value;
            signatures.push(self.body_signature(&path.0.package, public)?.0);
        }
        for signature in signatures {
            let hirsel_types::SignatureKind::Type(t) = &signature.kind else {
                continue;
            };
            let fields = t
                .representation
                .and_then(|id| signature.types.get(id))
                .map(|r| match &r.kind {
                    hirsel_types::TypeKind::Record(fs) => fs.len(),
                    hirsel_types::TypeKind::Variant(vs) => {
                        vs.iter().map(|v| v.fields.len()).max().unwrap_or(0)
                    }
                    _ => 1,
                })
                .unwrap_or(0);
            let finalizer = match &t.drop {
                Some(drop) if self.items.contains_key(drop) => {
                    let entry = if eager {
                        self.lower_to_tir(drop)?.value.entry
                    } else {
                        match &self.adapters.lowering {
                            Some(lowering) => lowering.entry_body(),
                            None => return self.missing(QueryKind::Lower),
                        }
                    };
                    Some((drop.clone(), entry))
                }
                // The registry's File/AtomicFile are native handles: Library's
                // drop_handle hook owns their cleanup, with no Hirsel body.
                Some(drop) if self.registry.get(drop).is_some() => None,
                Some(drop) => {
                    return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                        operation: "runtime types".into(),
                        message: format!("missing finalizer {drop:?}"),
                    }));
                }
                None => None,
            };
            runtime.nominals.push(hirsel_interp::aggregate::Nominal {
                definition: signature.definition.clone(),
                resource: t.resource,
                distinct: t.distinct,
                finalizer,
                fields,
                representation: t
                    .representation
                    .map(|shape| (signature.types.clone(), shape)),
            });
        }
        Ok(runtime)
    }
    pub fn manifest_revision(&self) -> ContentHash {
        ContentHash::of_parts(
            "hirsel.manifest.v1",
            [
                self.environment.package_manifest_hash.0.as_slice(),
                self.manifest_identity_hash.0.as_slice(),
            ],
        )
    }
}

impl QueryEngine {
    /// Whether `file` parsed cleanly into this engine's image.
    pub(crate) fn parsed_in_image(&self, file: FileId) -> bool {
        matches!(self.parsed.get(&file), Some((_, _, Ok(_))))
    }
    /// Parse `file` in this engine, discarding a retained outcome first.
    pub(crate) fn reparse_file(
        &mut self,
        file: FileId,
        context: &FileContext,
    ) -> Result<(), QueryError> {
        self.parsed.remove(&file);
        self.parse_file(file, context).map(|_| ())
    }
}
impl DefinitionQueries for QueryEngine {
    fn parse_file(
        &mut self,
        file: FileId,
        context: &FileContext,
    ) -> Result<QueryOutput<SyntaxFile>, QueryError> {
        let package_environment = self.package_environment(&context.package);
        if let Some((old_context, environment, result)) = self.parsed.get(&file) {
            if old_context == context && environment == &package_environment {
                return result.clone();
            }
        }
        let Some(lexer) = self.adapters.lexer.clone() else {
            return self.missing(QueryKind::Lex);
        };
        self.last_phase = Some(QueryKind::Lex);
        let tokens = lexer.lex(file, &self.sources, &mut self.symbols)?;
        if has_errors(&tokens.diagnostics) {
            let result = Err(QueryError::Diagnostics(tokens.diagnostics));
            self.parsed.insert(
                file,
                (context.clone(), package_environment.clone(), result.clone()),
            );
            return result;
        }
        let Some(parser) = self.adapters.parser.clone() else {
            return self.missing(QueryKind::Parse);
        };
        self.last_phase = Some(QueryKind::Parse);
        let mut output = match parser.parse(
            file,
            tokens.value,
            context,
            &self.sources,
            &self.symbols,
        ) {
            Ok(output) => output,
            Err(error) => {
                // This fixed implementation limit is deterministic for an exact
                // source snapshot. Retain only its failure, never partial syntax.
                if matches!(&error, QueryError::Operational(failure) if failure.operation == "parser-tooling-limit")
                {
                    self.parsed.insert(
                        file,
                        (context.clone(), package_environment, Err(error.clone())),
                    );
                }
                return Err(error);
            }
        };
        let mut diagnostics = tokens.diagnostics;
        diagnostics.append(&mut output.diagnostics);
        output.diagnostics = diagnostics;
        let result = if has_errors(&output.diagnostics) {
            Err(QueryError::Diagnostics(output.diagnostics.clone()))
        } else {
            self.index(&output.value, context).map(|_| output)
        };
        self.parsed.insert(
            file,
            (context.clone(), package_environment.clone(), result.clone()),
        );
        result
    }
    fn item_signature(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<hirsel_types::SignatureRef>, QueryError> {
        self.with_validation_walk(|engine| {
            engine
                .signature_query(definition)
                .map(|output| (*output).clone())
        })
    }
    fn check_definition(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<TypedBody>, QueryError> {
        self.check_query(definition).map(|output| (*output).clone())
    }
    fn lower_to_tir(
        &mut self,
        definition: &DefPath,
    ) -> Result<QueryOutput<hirsel_tir::DefinitionRef>, QueryError> {
        self.prepare_materialization_environment();
        let modes = self.mode_query(definition)?;
        let signature = self.signature_output(definition)?;
        let key = self.key(definition, QueryKind::Lower)?;
        self.read_execution_signatures(&modes.dependencies);
        if let Some(cached) = self.lowered.get(definition).cloned() {
            if self.valid(&cached, &key) {
                self.inherit_execution(&cached.executed);
                return cached.output();
            }
        }
        self.start(definition, QueryKind::Lower)?;
        let mut reads = self
            .modes
            .get(definition)
            .map(|c| c.reads.clone())
            .unwrap_or_default();
        reads.signatures.extend(modes.dependencies.clone());
        let mut body = (*modes).clone();
        let representations = self
            .modes
            .get(definition)
            .map(|c| c.representations.clone())
            .unwrap_or_default();
        self.execution_stack.push(ExecutionReads::default());
        let prepared = self.prepare_comptime(&mut body.value, &signature.value);
        let result = prepared.and_then(|()| match self.adapters.lowering.clone() {
            Some(adapter) => adapter.lower(body.value, &signature.value),
            None => self.missing(QueryKind::Lower),
        });
        let mut result = result.map(shared_output);
        if let Ok(lowered) = &result {
            let signatures = self.validated_signature_input(&definition.0.package);
            let report = signatures.validate_once(&lowered.value);
            if let Some(error) = hirsel_validate::tooling_limit_error(&report) {
                result = Err(error);
            } else if !report.violations.is_empty() {
                result = Err(QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "TIR validation".into(),
                    message: format!("{definition:?}: {:?}", report.violations),
                }));
            }
        }
        let executed = self
            .execution_stack
            .pop()
            .expect("lowering execution frame");
        let mut cached = self.finish(key, reads, result, representations);
        Arc::make_mut(&mut cached).executed = executed;
        if !cached.result.as_ref().is_err_and(|e| !e.cacheable()) {
            self.lowered.insert(definition.clone(), cached.clone());
        }
        self.publish_lowering(definition);
        cached.output()
    }
}
impl SignatureProvider for QueryEngine {
    fn signature(
        &mut self,
        definition: &DefPath,
    ) -> Result<hirsel_types::SignatureRef, QueryError> {
        self.with_validation_walk(|engine| {
            engine
                .signature_query(definition)
                .map(|output| output.value.clone())
        })
    }
    fn lookup(
        &mut self,
        context: &FileContext,
        path: &hirsel_syntax::Path,
        symbols: &Interner,
    ) -> hirsel_types::LookupOutput {
        hirsel_types::LookupOutput {
            manifest: ContentHash::of_parts(
                "hirsel.provider.registry.v1",
                [
                    self.manifest_revision().0.as_slice(),
                    self.registry.revision().0.as_slice(),
                ],
            ),
            result: (|| {
                if path.segments.iter().any(|segment| {
                    symbols
                        .resolve(*segment)
                        .is_some_and(|s| s.starts_with('$'))
                }) {
                    return Err(QueryError::NotImplemented);
                }
                let definition =
                    lookup_path(context, path, symbols).ok_or(QueryError::NotImplemented)?;
                if self.items.contains_key(&definition) || self.builtins.contains_key(&definition) {
                    Ok(definition)
                } else {
                    self.registry
                        .lookup_name(path, symbols)
                        .ok_or(QueryError::NotImplemented)
                }
            })(),
        }
    }
}
impl TirProvider for QueryEngine {
    fn constant(&mut self, path: &DefPath) -> Result<hirsel_tir::MaterializedRef, QueryError> {
        self.materialize_value(path).map(std::sync::Arc::new)
    }
    fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
        // #63: dependency-first; ties use source file path and declaration position.
        let mut declarations: Vec<_> = self
            .items
            .iter()
            .filter_map(|(path, items)| {
                let item = items.first()?;
                matches!(
                    item.file.tree.items[item.item.0 as usize].kind,
                    ItemKind::Const { .. }
                )
                .then(|| {
                    let site = item.file.tree.items[item.item.0 as usize].name_span;
                    (
                        self.sources.file(site.file).unwrap().path.clone(),
                        site.start,
                        path.clone(),
                    )
                })
            })
            .collect();
        declarations.sort();
        let mut paths: Vec<_> = self
            .registry
            .signatures()
            .filter(|s| matches!(s.kind, hirsel_types::SignatureKind::Const(_)))
            .map(|s| s.definition.clone())
            .collect();
        paths.extend(declarations.into_iter().map(|(_, _, path)| path));
        let ranks: BTreeMap<_, _> = paths
            .iter()
            .enumerate()
            .map(|(i, p)| (p.clone(), i))
            .collect();
        let mut users = vec![Vec::new(); paths.len()];
        let mut counts = vec![0; paths.len()];
        for (i, path) in paths.iter().enumerate() {
            let value = self.evaluate_constant(path)?;
            for dependency in value.executed.bodies.keys() {
                if let Some(&dependency) = ranks.get(dependency).filter(|&&d| d != i) {
                    users[dependency].push(i);
                    counts[i] += 1;
                }
            }
        }
        let mut ready: BTreeSet<_> = counts
            .iter()
            .enumerate()
            .filter_map(|(i, &n)| (n == 0).then_some(i))
            .collect();
        let mut ordered = Vec::with_capacity(paths.len());
        while let Some(i) = ready.pop_first() {
            ordered.push(paths[i].clone());
            for &user in &users[i] {
                counts[user] -= 1;
                if counts[user] == 0 {
                    ready.insert(user);
                }
            }
        }
        if ordered.len() != paths.len() {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "constant initialization".into(),
                message: "cyclic materialized constant dependencies".into(),
            }));
        }
        Ok(ordered)
    }

    fn definition(&mut self, path: &DefPath) -> Result<hirsel_tir::DefinitionRef, QueryError> {
        self.lower_to_tir(path).map(|o| o.value)
    }
}
struct Recorder<'a> {
    owner: DefPath,
    private_shapes: bool,
    representations: BTreeMap<DefPath, ContentHash>,
    engine: &'a mut QueryEngine,
    reads: DependencyReads,
}
impl SignatureProvider for Recorder<'_> {
    fn signature(
        &mut self,
        definition: &DefPath,
    ) -> Result<hirsel_types::SignatureRef, QueryError> {
        let result = self.engine.signature(definition);
        if let Err(QueryError::Operational(error)) = &result {
            self.reads.operational_failure = Some(error.clone());
        }
        match &result {
            Ok(signature) => self.reads.signatures.push(SignatureDependency {
                definition: definition.clone(),
                fingerprint: signature.fingerprint,
            }),
            Err(_) => {
                self.reads.failed_signatures.push(definition.clone());
                if definition != &self.owner
                    || !self
                        .engine
                        .active
                        .contains(&(definition.clone(), QueryKind::Signature))
                {
                    self.reads.failed_signature_keys.push((
                        definition.clone(),
                        self.engine.failed_signature_key(definition),
                    ));
                }
            }
        }
        if self.private_shapes {
            let public = result?;
            let (view, revision) = self.engine.body_signature(&self.owner.0.package, public)?;
            if let Some(revision) = revision {
                self.representations.insert(definition.clone(), revision);
            }
            Ok(view)
        } else {
            result
        }
    }
    fn lookup(
        &mut self,
        context: &FileContext,
        path: &hirsel_syntax::Path,
        symbols: &Interner,
    ) -> hirsel_types::LookupOutput {
        let result = self.engine.lookup(context, path, symbols);
        self.reads
            .manifest_lookups
            .push(hirsel_types::DependencyRead::ProviderLookup {
                context: context.clone(),
                package: path.package.clone(),
                segments: path
                    .segments
                    .iter()
                    .map(|s| symbols.resolve(*s).unwrap_or("<invalid symbol>").to_owned())
                    .collect(),
                output: result.clone(),
            });
        if let Err(QueryError::Operational(error)) = &result.result {
            self.reads.operational_failure = Some(error.clone());
        }
        if let Some(path) = lookup_path(context, path, symbols) {
            self.reads.lookups.push((path, result.result.is_ok()));
        }
        result
    }
}
fn lookup_path(
    context: &FileContext,
    path: &hirsel_syntax::Path,
    symbols: &Interner,
) -> Option<DefPath> {
    let mut segments: Vec<_> = path
        .segments
        .iter()
        .map(|s| symbols.resolve(*s).map(str::to_owned))
        .collect::<Option<_>>()?;
    let name = segments.pop()?;
    let modules = if path.package.is_none() && segments.is_empty() {
        context.modules.clone()
    } else {
        segments
    };
    Some(DefPath(ItemPath {
        package: path
            .package
            .clone()
            .unwrap_or_else(|| context.package.clone()),
        modules,
        name,
    }))
}
pub(crate) fn output<T>(value: T) -> QueryOutput<T> {
    QueryOutput {
        value,
        diagnostics: vec![],
        dependencies: vec![],
    }
}
pub(crate) fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(|d| d.severity == Severity::Error)
}

fn shared_output<T>(output: QueryOutput<T>) -> QueryOutput<std::sync::Arc<T>> {
    QueryOutput {
        value: std::sync::Arc::new(output.value),
        diagnostics: output.diagnostics,
        dependencies: output.dependencies,
    }
}

fn header_span(file: &SyntaxFile, item: &hirsel_syntax::Item) -> Span {
    let end = match item.kind {
        ItemKind::Function {
            body: Some(body), ..
        } => file.tree.expressions[body.0 as usize].origin.span.start,
        _ => item.origin.span.end,
    };
    Span {
        end,
        ..item.origin.span
    }
}
