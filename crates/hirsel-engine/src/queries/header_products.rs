//! Immutable source inputs and ordinary per-definition signature products.
//! Execution uses the scalar producer's stack-local recorder and canonical store
//! codec. No mutable query engine survives a production call.
use super::*;
use std::sync::{
    OnceLock, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
mod private;
mod sources;
use sources::HeaderSources;

type SignatureOutput = Result<Arc<QueryOutput<hirsel_types::SignatureRef>>, QueryError>;
pub(crate) struct HeaderProduct {
    pub(crate) result: SignatureOutput,
    pub(crate) entry: Option<Arc<Entry<hirsel_types::SignatureRef>>>,
    pub(crate) recomputes: usize,
    graph_work: usize,
    origin: Weak<Entry<hirsel_types::SignatureRef>>,
    accounted: AtomicBool,
}
#[derive(Clone)]
struct HeaderPhases {
    lexer: Option<Arc<dyn LexerPhase>>,
    parser: Option<Arc<dyn ParserPhase>>,
    signature: Option<Arc<dyn SignaturePhase>>,
    resolver: Option<Arc<dyn ResolverPhase>>,
    checker: Option<Arc<dyn CheckerPhase>>,
    modes: Option<Arc<dyn ModesPhase>>,
}
impl HeaderPhases {
    fn capture(adapters: &PhaseAdapters) -> Self {
        Self {
            lexer: adapters.lexer.clone(),
            parser: adapters.parser.clone(),
            signature: adapters.signature.clone(),
            resolver: adapters.resolver.clone(),
            checker: adapters.checker.clone(),
            modes: adapters.modes.clone(),
        }
    }
    fn adapters(&self) -> PhaseAdapters {
        PhaseAdapters {
            lexer: self.lexer.clone(),
            parser: self.parser.clone(),
            signature: self.signature.clone(),
            resolver: self.resolver.clone(),
            checker: self.checker.clone(),
            modes: self.modes.clone(),
            ..Default::default()
        }
    }
}
pub(crate) struct HeaderProducts {
    rows: hirsel_syntax::DefMap<OnceLock<HeaderProduct>>,
    generation: AtomicU64,
    retention: OnceLock<Arc<std::sync::Mutex<u64>>>,
    environment: QueryEnvironment,
    adapters: HeaderPhases,
    sources: SourceMap,
    owners: Arc<crate::NativeRecordContext>,
    provisional: bool,
    failure: Option<QueryError>,
    syntax: Arc<HeaderSources>,
    private: Option<private::PrivateProducts>,
    declarations: crate::DeclarationIndex,
    catalog: catalog::Catalog,
    types: Arc<hirsel_types::TypeInterner>,
    memo: Arc<hirsel_types::TypeMemo>,
    builtins: BTreeMap<DefPath, hirsel_types::SignatureRef>,
    registry: crate::registry::BuiltinRegistry,
    package_manifests: BTreeMap<hirsel_syntax::PackageId, ContentHash>,
    published_manifest: Option<ContentHash>,
    store: Option<Arc<dyn crate::store::ResultStore>>,
    identity: ContentHash,
    verify_store: crate::store::VerifyStore,
    limits: crate::ComptimeLimits,
    staged: BTreeMap<DefPath, crate::comptime::StagedConstant>,
    summaries: hirsel_syntax::DefMap<hirsel_types::UniqueSummary>,
    failures: BTreeMap<DefId, Vec<hirsel_types::UniqueFailure>>,
}
impl HeaderProducts {
    pub(crate) fn capture(engine: &QueryEngine) -> Arc<Self> {
        let rows = engine
            .source_paths()
            .into_iter()
            .map(|path| {
                let id = DefId::intern(&path);
                let cell = OnceLock::new();
                if engine.unmarked_signature(id).is_some()
                    && (engine.private_summary_revision == Some(engine.graph.revision())
                        || !engine
                            .source_facts(&path)
                            .is_some_and(|f| (f.body || f.generator) && !f.constant))
                {
                    let entry = engine.signatures[&id].clone();
                    let _ = cell.set(HeaderProduct {
                        result: entry.result(),
                        entry: Some(project(&entry)),
                        recomputes: 0,
                        graph_work: 0,
                        origin: Arc::downgrade(&entry),
                        accounted: AtomicBool::new(true),
                    });
                }
                (id, cell)
            })
            .collect();
        let owners = Arc::new(crate::NativeRecordContext::capture(engine));
        Arc::new(Self {
            rows,
            generation: AtomicU64::new(0),
            retention: OnceLock::new(),
            environment: engine.environment.clone(),
            adapters: HeaderPhases::capture(&engine.adapters),
            sources: engine.sources.clone(),
            owners: owners.clone(),
            provisional: engine.private_summary_active,
            failure: engine.current_private_summary_failure().cloned(),
            syntax: engine
                .header_products
                .as_ref()
                .map(|headers| headers.syntax.clone())
                .unwrap_or_else(|| Arc::new(HeaderSources::capture(engine))),
            private: private::PrivateProducts::capture(engine),
            declarations: engine.declarations.header_inputs(),
            catalog: engine.catalog.clone(),
            types: engine.types.clone(),
            memo: engine.memo.clone(),
            builtins: engine.builtins.clone(),
            registry: engine.registry.clone(),
            package_manifests: engine.package_manifests.clone(),
            published_manifest: engine.published_manifest,
            store: engine.result_store.clone(),
            identity: engine.store_identity,
            verify_store: engine.verify_store,
            limits: engine.comptime_limits,
            staged: engine.staged_constants.clone(),
            summaries: engine.private_summaries.clone(),
            failures: engine.private_unique_failures.clone(),
        })
    }
    fn execution(self: &Arc<Self>) -> QueryEngine {
        let mut engine =
            QueryEngine::new_with_hold_bytes(self.environment.clone(), self.adapters.adapters(), 0);
        engine.sources = self.sources.clone();
        engine.source_ownership = Arc::default();
        engine.private_summary_active = self.provisional;
        engine.private_summary_failure = self
            .failure
            .clone()
            .map(|error| (engine.graph.revision(), error));
        engine.items = native_records::SourceTable::new(engine.source_ownership.clone());
        engine.declarations = self.declarations.clone();
        engine.catalog = self.catalog.clone();
        engine
            .catalog
            .bind_ownership(engine.source_ownership.clone());
        engine.types = self.types.clone();
        engine.memo = self.memo.clone();
        engine.builtins = self.builtins.clone();
        engine.registry = self.registry.clone();
        engine.package_manifests = self.package_manifests.clone();
        engine.published_manifest = self.published_manifest;
        engine.result_store = self.store.clone();
        engine.store_identity = self.identity;
        engine.verify_store = self.verify_store;
        engine.comptime_limits = self.limits;
        engine.staged_constants = self.staged.clone();
        engine.header_products = Some(self.clone());
        engine
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }
    pub(crate) fn source_environment(&self) -> &QueryEnvironment {
        &self.environment
    }
    pub(crate) fn source_types(&self) -> &Arc<hirsel_types::TypeInterner> {
        &self.types
    }
    pub(crate) fn work_snapshot(&self) -> Vec<(DefPath, QueryKind, u64)> {
        self.completed()
            .map(|(id, product)| {
                (
                    id.path().clone(),
                    QueryKind::Signature,
                    product.graph_work as u64,
                )
            })
            .collect()
    }
    pub(crate) fn definitions(&self) -> impl Iterator<Item = DefId> + '_ {
        self.rows.keys().copied()
    }
    /// Freeze input transport under the front host's existing source grant.
    /// Legacy scalar views keep their written immutable inputs; a disposable
    /// result provider never becomes their required source-input owner.
    pub(crate) fn archive_sources(
        &self,
        store: Arc<dyn crate::store::ResultStore>,
    ) -> Result<(), QueryError> {
        self.syntax
            .archive(self.types.clone(), self.owners.clone(), store)
    }
    pub(super) fn summary(&self, id: DefId) -> Option<&hirsel_types::UniqueSummary> {
        self.demanded_summary(id)
            .or_else(|| self.summaries.get(&id))
    }
    pub(crate) fn owners(&self) -> &Arc<crate::NativeRecordContext> {
        &self.owners
    }
    pub(crate) fn materialize(
        &self,
        engine: &mut QueryEngine,
        path: &DefPath,
    ) -> Result<bool, QueryError> {
        self.syntax.materialize(engine, path)
    }
    pub(crate) fn contains(&self, id: DefId) -> bool {
        self.rows.contains_key(&id)
    }
    pub(crate) fn product(self: &Arc<Self>, id: DefId) -> Option<&HeaderProduct> {
        let cell = self.rows.get(&id)?;
        if cell.get().is_none() {
            let mut engine = self.execution();
            if let Err(error) = self.demand(&mut engine, id) {
                let _ = cell.set(HeaderProduct {
                    result: Err(error),
                    entry: None,
                    recomputes: 0,
                    graph_work: 0,
                    origin: Weak::new(),
                    accounted: AtomicBool::new(false),
                });
            }
        }
        cell.get()
    }
    pub(crate) fn demand(self: &Arc<Self>, engine: &mut QueryEngine, id: DefId) -> SignatureOutput {
        self.private_product(id)?;
        if engine.header_products.is_none() {
            self.adopt_private_products(engine)?;
        }
        if let Some(failures) = self.failures.get(&id) {
            engine
                .private_unique_failures
                .entry(id)
                .or_insert_with(|| failures.clone());
        }
        let Some(cell) = self.rows.get(&id) else {
            return engine.signature_query_local(id);
        };
        if engine
            .active
            .contains(&(id.path().clone(), QueryKind::Signature))
        {
            return engine.signature_query_local(id);
        }
        if let Some(product) = cell.get() {
            if engine.header_products.is_none() {
                engine.adopt_header_product(id, product);
            } else if let Some(entry) = &product.entry {
                engine.graph.adopt(
                    entry.memo.key,
                    entry.memo.reads.clone(),
                    entry.memo.durability,
                );
                engine.signatures.insert(id, entry.clone());
            }
            return product.result.clone();
        }
        let result = engine.signature_query_local(id);
        let source = engine.signatures.get(&id);
        let local = engine.header_products.is_none();
        let origin = source
            .filter(|_| local)
            .map_or_else(Weak::new, Arc::downgrade);
        let entry = source.map(project);
        // Never wait while holding a recursive header stack. Concurrent pure
        // normalizations race only to publish the same immutable source product.
        let recomputes = engine.recompute_count(id.path(), QueryKind::Signature);
        let bytes = entry
            .as_ref()
            .and_then(|entry| crate::memo::retained::retained_signature_entry_bytes(entry))
            .unwrap_or(0);
        let mut grant = self
            .retention
            .get()
            .map(|grant| grant.lock().unwrap_or_else(|e| e.into_inner()));
        if let Some(grant) = &mut grant {
            **grant = grant.checked_sub(bytes).ok_or_else(|| {
                QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "header admission".into(),
                    message: "demanded header exceeds the existing front partition".into(),
                })
            })?;
        }
        if cell
            .set(HeaderProduct {
                result,
                entry,
                recomputes,
                graph_work: if local { 0 } else { recomputes },
                origin,
                accounted: AtomicBool::new(local),
            })
            .is_ok()
        {
            self.generation.fetch_add(1, Ordering::Relaxed);
        } else if let Some(grant) = &mut grant {
            **grant += bytes;
        }
        cell.get()
            .expect("a completed producer publishes its row")
            .result
            .clone()
    }
    pub(crate) fn ready(&self, id: DefId) -> Option<&HeaderProduct> {
        self.rows.get(&id)?.get()
    }
    pub(crate) fn pending_recomputes(&self, id: DefId) -> usize {
        self.ready(id)
            .filter(|p| !p.accounted.load(Ordering::Relaxed))
            .map_or(0, |p| p.recomputes)
    }
    pub(crate) fn completed(&self) -> impl Iterator<Item = (DefId, &HeaderProduct)> {
        self.rows
            .iter()
            .filter_map(|(id, cell)| cell.get().map(|product| (*id, product)))
    }
}

impl QueryEngine {
    pub(crate) fn adopt_completed_headers(&mut self) {
        let Some((stamp, snapshot)) = &self.snapshot else {
            return;
        };
        let current = *stamp == self.snapshot_stamp();
        let Some(headers) = snapshot.header_products().cloned() else {
            return;
        };
        if current {
            let _ = headers.adopt_private_products(self);
            self.adopt_header_products(&headers);
        } else {
            for (id, product) in headers.completed() {
                self.account_header_work(id, product);
            }
        }
    }
    pub(crate) fn adopt_header_products(&mut self, headers: &header_products::HeaderProducts) {
        for (id, product) in headers.completed() {
            self.adopt_header_product(id, product);
        }
    }
    fn account_header_work(&mut self, id: DefId, product: &HeaderProduct) {
        if !product.accounted.swap(true, Ordering::Relaxed) {
            *self
                .recomputes
                .entry((id.path().clone(), QueryKind::Signature))
                .or_default() += product.recomputes;
        }
    }
    fn adopt_header_product(&mut self, id: DefId, product: &HeaderProduct) {
        self.account_header_work(id, product);
        let Some(entry) = &product.entry else {
            return;
        };
        if self.signatures.get(&id).is_some_and(|current| {
            Arc::ptr_eq(current, entry)
                || product
                    .origin
                    .upgrade()
                    .is_some_and(|origin| Arc::ptr_eq(current, &origin))
        }) {
            return;
        }
        self.graph.adopt(
            entry.memo.key,
            entry.memo.reads.clone(),
            entry.memo.durability,
        );
        self.signatures.insert(id, entry.clone());
        if self.private_summary_active {
            self.private_signature_publications.insert(id);
        } else {
            self.private_signature_publications.remove(&id);
        }
        self.snapshot_sources.insert(id, Arc::downgrade(entry));
    }
}

fn project(
    entry: &Arc<Entry<hirsel_types::SignatureRef>>,
) -> Arc<Entry<hirsel_types::SignatureRef>> {
    if entry.products.resolution.is_none() {
        return entry.clone();
    }
    Arc::new(Entry {
        memo: entry.memo.clone(),
        detail: entry.detail.clone(),
        products: Arc::new(PhaseProducts {
            resolution: None,
            ..(*entry.products).clone()
        }),
        covered: entry.covered.clone(),
        depth: entry.depth.clone(),
        acceptance: entry.acceptance.clone(),
        verification: Default::default(),
        selections: entry.selections.clone(),
        consistent: entry.consistent,
    })
}
