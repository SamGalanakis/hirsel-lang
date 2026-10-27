//! Immutable launch facts from the admitted source semantic closure.
use super::*;
mod content;
mod proof_export;
use hirsel_run_schema::{CallForm, CalleeFacts, CodeReach, ProgramRevision};
pub use proof_export::{
    FrontBuiltProof, FrontPreparedProof, FrontProofExport, FrontProofInput, FrontProofText,
};
/// Metadata consumed by interpreter/H/native hosts. No mutable query engine,
/// body cache or native Value reconstruction is available through this output.
pub struct FrontRunOutput {
    pub definition_hashes: Option<Arc<BTreeMap<DefPath, ContentHash>>>,
    pub entry: DefPath,
    pub revision: ProgramRevision,
    pub code: CodeReach,
    pub foreign: ForeignUse,
    pub semantic: Arc<crate::NativeSemanticProgram>,
    pub callees: BTreeMap<String, CalleeFacts>,
    pub entries: BTreeMap<String, (CalleeFacts, CallForm)>,
    pub runtime: Arc<hirsel_interp::aggregate::RuntimeTypes>,
    pub checked_runtime: Arc<hirsel_interp::checked::CheckedRuntime>,
    pub(crate) _owner: Arc<dyn Send + Sync>,
}

use crate::runtime::*;
use crate::store::{Domain, Reference, ResultStore};
use hirsel_proof_export::DefinitionContent;
use hirsel_types::{SignatureKind, SignatureRef};
use std::{sync::Mutex, task::Poll};

pub const FRONT_RUN_KIND: u32 = 0x4600_0010;
pub const FRONT_RUN_BATCH_KIND: u32 = 0x4600_001e;
const BATCH_OUTPUT: Domain = Domain {
    kind: 0x4600_001f,
    schema: 1,
};
pub type FrontRunBatchOutput = BTreeMap<DefPath, Arc<FrontRunOutput>>;
const OUTPUT: Domain = Domain {
    kind: 0x4600_0011,
    schema: 1,
};
const FAILURE: Domain = Domain {
    kind: 0x4600_0012,
    schema: 1,
};
// Canonical source bytes use the request's existing bounded transport. The
// immutable catalogue retains descriptors; a demanded node decodes its bytes
// under its scratch grant, just like archived ordinary front syntax.
const INPUT_CONTENT: Domain = Domain {
    kind: 0x4600_001d,
    schema: 1,
};
struct Input {
    signature: SignatureRef,
    owner_signature: SignatureRef,
    content: Reference,
    dependencies: BTreeSet<DefPath>,
    source: bool,
    executable: bool,
    generated: bool,
    implementation: ContentHash,
}
/// Prepared immutable Run inputs and their exact charged bytes. The private
/// catalog moves into the producer once, so sizing never repeats encoding or
/// rereads a newer source snapshot. This carries no V or native C authority.
pub struct FrontRunInputs {
    signatures: Arc<crate::snapshot::SignatureSnapshot>,
    source_paths: Vec<DefPath>,
    inputs: BTreeMap<DefPath, Input>,
    views: Vec<(SignatureRef, ContentHash)>,
    store: Arc<dyn ResultStore>,
    required: u64,
}
impl FrontRunInputs {
    pub fn input_bytes(&self) -> u64 {
        self.required
    }
}

/// One immutable input namespace, shared across selected Run roots. Each task
/// waits for the ordered source transaction before reading semantic products.
pub struct FrontRunQueries {
    batch: Option<(QueryId, Vec<DefPath>)>,
    completed_batch: Mutex<Option<(ProductRef, Arc<FrontRunBatchOutput>)>>,
    export_hashes: bool,
    native_roots: Vec<DefPath>,
    inputs: BTreeMap<DefPath, Input>,
    ids: BTreeMap<DefPath, QueryId>,
    check: Arc<FrontCheckQueries>,
    front: Arc<FrontBodyQueries>,
    lower: Arc<FrontLowerQueries>,
    runtime: Arc<hirsel_interp::aggregate::RuntimeTypes>,
    foreign_threads: BTreeMap<hirsel_syntax::PackageId, hirsel_types::ForeignThreads>,
    foreign_links: Vec<LinkInput>,
    store: Arc<dyn ResultStore>,
    records: crate::NativeRecords,
    anchors: crate::NativeRecordContext,
    completed: Mutex<BTreeMap<ContentHash, (ProductRef, Arc<FrontRunOutput>)>>,
    failures: Mutex<BTreeMap<ContentHash, (Reference, QueryError)>>,
    retention: Mutex<u64>,
    weight: MemoryWeight,
    errors: RuntimeErrors,
    ownership: Arc<Ownership>,
}
struct Ownership {
    _front: Arc<FrontBodyQueries>,
    _lower: Arc<FrontLowerQueries>,
    _allowance: MemoryReservation,
}
impl FrontRunQueries {
    /// One source closure and canonical hash pass for a bounded set of launches.
    /// Each entry still has its own semantic revision, reach and callable facts.
    pub fn with_batch_roots(
        mut self: Arc<Self>,
        roots: Vec<DefPath>,
    ) -> Result<Arc<Self>, QueryError> {
        if roots.is_empty() {
            return Err(operational("empty Run batch"));
        }
        self = self.with_source_roots(roots)?;
        let this = Arc::get_mut(&mut self)
            .ok_or_else(|| operational("batch requires an unshared Run view"))?;
        let roots = this.native_roots.clone();
        let bytes = serde_json::to_vec(&roots).map_err(|error| operational(error.to_string()))?;
        let charge = (bytes.len() as u64)
            .checked_add((roots.len() * std::mem::size_of::<DefPath>()) as u64)
            .ok_or_else(|| operational("Run batch inventory overflow"))?;
        let remaining = this
            .retention
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        *remaining = remaining
            .checked_sub(charge)
            .ok_or_else(|| operational("Run batch roots exceed admission"))?;
        let query = QueryId {
            kind: FRONT_RUN_BATCH_KIND,
            address: hirsel_artifact_store::Address(ContentHash::of_parts(
                "hirsel.front.run.batch.v1",
                [this.ids[&roots[0]].address.0.0.as_slice(), bytes.as_slice()],
            )),
        };
        this.batch = Some((query, roots));
        Ok(self)
    }
    pub fn batch_query(&self) -> Option<QueryId> {
        self.batch.as_ref().map(|(query, _)| *query)
    }
    pub fn batch_roots(&self) -> Option<&[DefPath]> {
        self.batch.as_ref().map(|(_, roots)| roots.as_slice())
    }
    pub fn batch_outcome(&self, product: ProductRef) -> Option<Arc<FrontRunBatchOutput>> {
        self.completed_batch
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .and_then(|(selected, output)| (*selected == product).then(|| output.clone()))
    }
    /// Select LANG-6 hashes for an immutable package closure. Ordinary execution
    /// does not retain this diagnostic map.
    pub fn with_definition_hashes(
        mut self: Arc<Self>,
        roots: Vec<DefPath>,
    ) -> Result<Arc<Self>, QueryError> {
        self = self.with_source_roots(roots)?;
        let this = Arc::get_mut(&mut self)
            .ok_or_else(|| operational("hash selection requires an unshared Run view"))?;
        this.export_hashes = true;
        for id in this.ids.values_mut() {
            id.address = hirsel_artifact_store::Address(ContentHash::of_parts(
                "hirsel.front.run.definition-hashes.v1",
                [id.address.0.0.as_slice()],
            ));
        }
        Ok(self)
    }

    /// Native selects the union of its source roots before this immutable view
    /// is shared with any factory. Ordinary launch identities stay rooted at
    /// the requested entry; the semantic handoff carries the selected union.
    pub(crate) fn with_source_roots(
        mut self: Arc<Self>,
        mut roots: Vec<DefPath>,
    ) -> Result<Arc<Self>, QueryError> {
        roots.sort();
        roots.dedup();
        if roots.iter().any(|path| !self.ids.contains_key(path)) {
            return Err(operational(
                "native source root is outside the captured namespace",
            ));
        }
        let bytes = serde_json::to_vec(&roots).map_err(|e| operational(e.to_string()))?;
        let this = Arc::get_mut(&mut self)
            .ok_or_else(|| operational("native source roots require an unshared Run view"))?;
        let charge = (bytes.len() as u64)
            .checked_add((roots.len() * std::mem::size_of::<DefPath>()) as u64)
            .ok_or_else(|| operational("native root inventory overflow"))?;
        let remaining = this.retention.get_mut().unwrap_or_else(|e| e.into_inner());
        *remaining = remaining
            .checked_sub(charge)
            .ok_or_else(|| operational("native source roots exceed Run admission"))?;
        for id in this.ids.values_mut() {
            id.address = hirsel_artifact_store::Address(ContentHash::of_parts(
                "hirsel.front.run.source-root-set.v1",
                [id.address.0.0.as_slice(), bytes.as_slice()],
            ));
        }
        this.native_roots = roots;
        Ok(self)
    }
    pub(crate) fn matches_inputs(
        &self,
        front: &Arc<FrontBodyQueries>,
        lower: &Arc<FrontLowerQueries>,
    ) -> bool {
        Arc::ptr_eq(&self.front, front) && Arc::ptr_eq(&self.lower, lower)
    }
    /// Potential root cursors and bodyless header/semantic-node slots. Exact
    /// canonical input content is charged by the constructor in addition.
    pub fn input_bytes(definitions: &[DefPath]) -> Option<u64> {
        definitions
            .iter()
            .try_fold(std::mem::size_of::<Self>() as u64, |n, path| {
                n.checked_add(bincode::serialized_size(path).ok()?.checked_mul(4)?)?
                    .checked_add(
                        (std::mem::size_of::<Input>() + 16 * std::mem::size_of::<usize>()) as u64,
                    )
            })
    }
    pub fn output_bytes(weight: MemoryWeight) -> Option<u64> {
        weight
            .retained
            .checked_add(std::mem::size_of::<FrontRunOutput>() as u64)
    }
    pub fn query(&self, path: &DefPath) -> Option<QueryId> {
        self.ids.get(path).copied()
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    pub(crate) fn completed_product(&self, id: QueryId) -> Option<ProductRef> {
        self.completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id.address.0)
            .map(|(product, _)| *product)
    }
    pub fn outcome(&self, product: ProductRef) -> Option<Arc<FrontRunOutput>> {
        let values = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        let (selected, output) = values.get(&product.address.0)?;
        (*selected == product).then(|| output.clone())
    }
    pub fn failure(&self, reference: Reference) -> Option<QueryError> {
        let values = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let (selected, error) = values.get(&reference.output.0)?;
        (*selected == reference).then(|| error.clone())
    }
    fn failure_record(&self, error: QueryError) -> Reference {
        let Some(bytes) = self.records.encode(&self.anchors, &error) else {
            return self.errors.resource;
        };
        if bytes.len() as u64 > self.weight.retained {
            return self.errors.resource;
        }
        let Some(reference) = self.store.put_content(FAILURE, &bytes) else {
            return self.errors.resource;
        };
        self.failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(reference.output.0, (reference, error));
        reference
    }
}
struct Factory(Arc<FrontRunQueries>);
impl QueryFactory for Factory {
    fn create(&self, query: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        let (root, completed) = if let Some((batch, roots)) = &self.0.batch
            && query == *batch
        {
            (
                roots[0].clone(),
                self.0
                    .completed_batch
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .as_ref()
                    .map(|(product, _)| *product),
            )
        } else {
            let root = self
                .0
                .ids
                .iter()
                .find_map(|(path, id)| (*id == query).then(|| path.clone()))
                .ok_or(self.0.errors.protocol)?;
            let completed = self
                .0
                .completed
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&query.address.0)
                .map(|(product, _)| *product);
            (root, completed)
        };
        // A root's potentially retained graph is admitted before any work.
        let mut retention = self.0.retention.lock().unwrap_or_else(|e| e.into_inner());
        let bytes = FrontRunQueries::output_bytes(self.0.weight).ok_or(self.0.errors.resource)?;
        if completed.is_none() {
            *retention = retention.checked_sub(bytes).ok_or(self.0.errors.resource)?;
        }
        drop(retention);
        Ok(Box::new(Task {
            queries: self.0.clone(),
            query,
            root,
            checked: false,
            pending: Vec::new(),
            entries: BTreeMap::new(),
            waiting: None,
            terminal: completed.map(Ok),
        }))
    }
}
struct GraphEntry {
    node: content::Node,
    content: content::Content,
}
struct Task {
    queries: Arc<FrontRunQueries>,
    query: QueryId,
    root: DefPath,
    checked: bool,
    pending: Vec<DefId>,
    entries: BTreeMap<DefId, Box<GraphEntry>>,
    waiting: Option<QueryId>,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.query
    }
    fn weight(&self) -> MemoryWeight {
        self.queries.weight
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        let result = if let Some(result) = self.terminal {
            result
        } else {
            match self.advance(cx) {
                Poll::Pending => return QueryPoll::Waiting(self.waiting.into_iter().collect()),
                Poll::Ready(result) => {
                    self.terminal = Some(result);
                    result
                }
            }
        };
        match result {
            Ok(product) => QueryPoll::Complete(product),
            Err(error) => QueryPoll::Failed(error),
        }
    }
}
impl Task {
    fn read(
        &mut self,
        cx: &mut dyn QueryContext,
        query: QueryId,
    ) -> Poll<Result<ProductRef, Reference>> {
        match cx.read(query) {
            Poll::Pending => {
                self.waiting = Some(query);
                Poll::Pending
            }
            Poll::Ready(result) => {
                self.waiting = None;
                Poll::Ready(result)
            }
        }
    }
    fn advance(&mut self, cx: &mut dyn QueryContext) -> Poll<Result<ProductRef, Reference>> {
        if !self.checked {
            let product = match self.read(cx, self.queries.check.query()) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(product)) => product,
            };
            let Some(output) = self.queries.check.outcome(product) else {
                return Poll::Ready(Err(self.queries.errors.protocol));
            };
            if output.checked.completion != CheckCompletion::Complete {
                return Poll::Ready(Err(self.queries.failure_record(operational(format!(
                    "Run source check is incomplete: {:?}",
                    output.checked.completion
                )))));
            }
            if has_errors(&output.checked.diagnostics) {
                return Poll::Ready(Err(self
                    .queries
                    .failure_record(QueryError::Diagnostics(output.checked.diagnostics))));
            }
            self.checked = true;
            self.pending.push(DefId::intern(&self.root));
            self.pending
                .extend(self.queries.native_roots.iter().map(DefId::intern));
            // Legacy served_entries selects every eligible source entry in the
            // launch package, even when the root never references it.
            for (path, input) in &self.queries.inputs {
                if path.0.package == self.root.0.package
                    && input.source
                    && crate::admission::entry_roots(
                        self.queries.lower.signatures().types(),
                        &input.signature,
                        hirsel_run_schema::LaunchKind::Call,
                    )
                    .is_ok()
                {
                    self.pending.push(DefId::intern(path));
                }
            }
            // Runtime type construction historically demands all declared drop
            // bodies. These are explicit S5 dependencies, never host work.
            for nominal in &self.queries.runtime.nominals {
                if let Some((path, _)) = &nominal.finalizer {
                    self.pending.push(DefId::intern(path));
                }
            }
        }
        loop {
            while let Some(id) = self.pending.last().copied() {
                let path = id.path();
                if self.entries.contains_key(&DefId::intern(path)) {
                    self.pending.pop();
                    continue;
                }
                match self.node(cx, path) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Ready(Ok((node, content, dependencies))) => {
                        self.pending.pop();
                        self.pending.extend(
                            dependencies
                                .iter()
                                .filter(|p| !self.entries.contains_key(&DefId::intern(p)))
                                .map(DefId::intern),
                        );
                        let content = match content::Content::new(&self.queries, content) {
                            Ok(content) => content,
                            Err(error) => {
                                return Poll::Ready(Err(self.queries.failure_record(error)));
                            }
                        };
                        self.entries
                            .insert(id, Box::new(GraphEntry { node, content }));
                    }
                }
                // Canonical input bytes plus graph rows and decoded working bodies
                // are bounded by this task's retained/scratch admission.
                if self
                    .graph_bytes()
                    .is_none_or(|bytes| bytes > self.queries.weight.retained)
                {
                    return Poll::Ready(Err(self.queries.errors.resource));
                }
            }
            match self.queries.lower.selected_instances(
                self.entries.keys().map(|id| id.path().clone()),
                self.queries.weight.scratch,
            ) {
                Ok(instances) => self.pending.extend(
                    instances
                        .into_iter()
                        .filter(|p| !self.entries.contains_key(&DefId::intern(p)))
                        .map(|path| DefId::intern(&path)),
                ),
                Err(error) => return Poll::Ready(Err(self.queries.failure_record(error))),
            }
            if self.pending.is_empty() {
                break;
            }
        }
        Poll::Ready(
            self.complete()
                .map_err(|error| self.queries.failure_record(error)),
        )
    }
    fn node(
        &mut self,
        cx: &mut dyn QueryContext,
        path: &DefPath,
    ) -> Poll<Result<(content::Node, DefinitionContent, BTreeSet<DefPath>), Reference>> {
        let queries = self.queries.clone();
        let Some(input) = queries.inputs.get(path) else {
            return Poll::Ready(Err(queries.failure_record(operational(format!(
                "missing signature input for {path}"
            )))));
        };
        let mut dependencies = input.dependencies.clone();
        let mut tir = None;
        let mut generated_tokens = None;
        if input.source {
            let Some(id) = queries.front.query(path) else {
                return Poll::Ready(Err(
                    queries.failure_record(operational(format!("missing P input for {path}")))
                ));
            };
            let product = match self.read(cx, id) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(product)) => product,
            };
            let output = match queries.front.outcome(product.record) {
                Some(output) => output,
                None => {
                    let Some(id) = queries.front.body_query(path) else {
                        return Poll::Ready(Err(queries.errors.protocol));
                    };
                    let product = match self.read(cx, id) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                        Poll::Ready(Ok(product)) => product,
                    };
                    let Some(output) = queries.front.outcome(product.record) else {
                        return Poll::Ready(Err(queries.errors.protocol));
                    };
                    output
                }
            };
            let analyzed = match &output.outcome.result {
                Ok(body) => body,
                Err(error) => return Poll::Ready(Err(queries.failure_record(error.clone()))),
            };
            if has_errors(&analyzed.diagnostics) {
                return Poll::Ready(Err(
                    queries.failure_record(QueryError::Diagnostics(analyzed.diagnostics.clone()))
                ));
            }
            if input.generated {
                generated_tokens = match hirsel_fmt::semantic_item(&analyzed.value.typed.syntax) {
                    Some(tokens) => Some(tokens),
                    None => {
                        return Poll::Ready(
                            Err(queries.failure_record(QueryError::NotImplemented)),
                        );
                    }
                };
            }
            dependencies.extend(
                analyzed
                    .dependencies
                    .iter()
                    .map(|d| d.definition.path().clone()),
            );
            extend_reads(&mut dependencies, &output.outcome.reads, |key| {
                queries.front.lookup_definition(key)
            });
            if input.executable {
                let Some(id) = queries.lower.query(path) else {
                    return Poll::Ready(Err(queries
                        .failure_record(operational(format!("missing Lower input for {path}")))));
                };
                let product = match self.read(cx, id) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Ready(Ok(product)) => product,
                };
                let Some(output) = queries.lower.outcome(product) else {
                    return Poll::Ready(Err(queries.failure_record(operational(format!(
                        "missing settled Lower product for {path}"
                    )))));
                };
                if has_errors(&output.output.diagnostics) {
                    return Poll::Ready(Err(
                        queries.failure_record(QueryError::Diagnostics(output.output.diagnostics))
                    ));
                }
                dependencies.extend(
                    output
                        .output
                        .dependencies
                        .iter()
                        .map(|d| d.definition.path().clone()),
                );
                extend_reads(&mut dependencies, &output.reads, |key| {
                    queries.front.lookup_definition(key)
                });
                tir = Some(output.output.value);
                if matches!(&input.signature.kind, SignatureKind::Const {generics, ..} if generics.is_empty())
                {
                    let Some(id) = queries.lower.evaluate_query(path) else {
                        return Poll::Ready(Err(queries.failure_record(operational(format!(
                            "missing Evaluate input for {path}"
                        )))));
                    };
                    match self.read(cx, id) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                        Poll::Ready(Ok(_)) => {}
                    }
                    if let Some(reads) = queries.lower.settled_reads(path, QueryKind::Evaluate) {
                        extend_reads(&mut dependencies, &reads, |key| {
                            queries.front.lookup_definition(key)
                        });
                    }
                    let value = match queries.lower.settled_constant(path, &[]) {
                        Ok(value) => value,
                        Err(error) => return Poll::Ready(Err(queries.failure_record(error))),
                    };
                    let mut pending = vec![&value.root];
                    while let Some(value) = pending.pop() {
                        match &value.kind {
                            hirsel_tir::ValueKind::FunctionReference { definition, .. } => {
                                dependencies.insert(definition.clone());
                            }
                            hirsel_tir::ValueKind::Construct { fields, .. } => {
                                pending.extend(fields)
                            }
                            _ => (),
                        }
                    }
                }
            }
        }
        let source_content = (|| -> Result<Vec<Vec<u8>>, QueryError> {
            use bincode::Options;
            if input.content.len > queries.weight.scratch {
                return Err(operational("Run input content exceeds scratch admission"));
            }
            let bytes = queries
                .store
                .get_content(input.content)
                .ok_or_else(|| operational("missing Run input content"))?;
            if INPUT_CONTENT.reference(&bytes) != Some(input.content) {
                return Err(operational(
                    "Run input content differs from captured descriptor",
                ));
            }
            bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .with_limit(input.content.len)
                .reject_trailing_bytes()
                .deserialize(&bytes)
                .map_err(|_| operational("invalid Run input content"))
        })();
        let mut source_content = match source_content {
            Ok(content) => content,
            Err(error) => return Poll::Ready(Err(queries.failure_record(error))),
        };
        if let Some(tokens) = generated_tokens {
            // Scalar semantic_body_node hashes the analyzed source: a generic
            // generated header has no body, and a selected instance carries
            // its accepted literal rather than the original generator call.
            source_content.truncate(1);
            source_content.extend(tokens);
        }
        let content = if matches!(input.signature.kind, SignatureKind::Type(_)) {
            DefinitionContent::Body {
                signature: input.owner_signature.clone(),
                tir: None,
            }
        } else if input.executable {
            DefinitionContent::Body {
                signature: input.signature.clone(),
                tir,
            }
        } else {
            DefinitionContent::Named {
                signature: input.signature.clone(),
                source: if input.source {
                    source_content.clone()
                } else {
                    vec![]
                },
                reads: if input.source {
                    dependencies.iter().cloned().collect()
                } else {
                    vec![]
                },
            }
        };
        // Definition hashes follow the existing proof-export reference walker;
        // semantic revision follows the independently captured semantic reads.
        let references =
            hirsel_proof_export::references(queries.lower.signatures().types(), &content);
        let mut all_dependencies = dependencies.clone();
        all_dependencies.extend(references);
        let node = match content::Node::new(&queries, source_content, &dependencies) {
            Ok(node) => node,
            Err(error) => return Poll::Ready(Err(queries.failure_record(error))),
        };
        Poll::Ready(Ok((node, content, all_dependencies)))
    }
    fn graph_bytes(&self) -> Option<u64> {
        let mut metadata = crate::NativeMetadataWeight::default();
        metadata.tree::<DefId, Box<GraphEntry>>(self.entries.len());
        metadata.flat_vec(&self.pending);
        for entry in self.entries.values() {
            metadata.add(std::mem::size_of::<GraphEntry>() + 2 * std::mem::size_of::<usize>());
            entry.node.weight(&mut metadata);
            entry.content.weight(&mut metadata);
        }
        Some(metadata.bytes())
    }

    fn program(&self, root: &DefPath) -> Result<SemanticProgram, QueryError> {
        self.program_from(vec![root.clone()])
    }
    fn program_from(&self, mut pending: Vec<DefPath>) -> Result<SemanticProgram, QueryError> {
        let mut definitions = BTreeSet::new();
        loop {
            while let Some(path) = pending.pop() {
                if definitions.insert(path.clone()) {
                    pending.extend(
                        self.entries
                            .get(&DefId::intern(&path))
                            .into_iter()
                            .flat_map(|entry| entry.node.dependencies.iter())
                            .map(|id| id.path().clone()),
                    );
                }
            }
            pending.extend(
                self.queries
                    .lower
                    .selected_instances(definitions.iter().cloned(), self.queries.weight.scratch)?
                    .into_iter()
                    .filter(|p| !definitions.contains(p)),
            );
            if pending.is_empty() {
                break;
            }
        }
        let mut nodes = vec![super::program_revision::runtime_node()];
        for path in &definitions {
            if let Some(node) = self.entries.get(&DefId::intern(path)) {
                nodes.push(node.node.program(&self.queries, path)?);
            }
        }
        Ok(SemanticProgram {
            nodes,
            uses: definitions
                .iter()
                .filter_map(|p| {
                    self.entries.get(&DefId::intern(p)).map(|entry| {
                        (
                            p.clone(),
                            entry
                                .node
                                .dependencies
                                .iter()
                                .map(|id| id.path().clone())
                                .collect(),
                        )
                    })
                })
                .collect(),
            definitions,
        })
    }
    fn complete(&self) -> Result<ProductRef, QueryError> {
        let queries = &self.queries;
        let snapshot = queries.lower.signatures();
        let mut references = crate::NativeMetadataWeight::default();
        references.tree::<DefPath, Vec<DefPath>>(self.entries.len());
        for (id, entry) in &self.entries {
            references.path(id.path());
            entry.content.reference_bytes(&mut references);
        }
        let decode_allowance = queries
            .weight
            .scratch
            .checked_sub(references.bytes())
            .ok_or_else(|| operational("Run reference map exceeds scratch admission"))?;
        let referenced = self
            .entries
            .iter()
            .map(|(id, entry)| (id.path().clone(), entry.content.references()))
            .collect();
        let hashes = Arc::new(
            hirsel_proof_export::definition_hashes_loading(snapshot.types(), &referenced, |path| {
                self.entries[&DefId::intern(path)]
                    .content
                    .load(queries, path, decode_allowance)
                    .map_err(|error| format!("{error:?}"))
            })
            .map_err(operational)?,
        );
        let checked_runtime = Arc::new(snapshot.checked_runtime(queries.runtime.forget.as_ref())?);
        let Some((batch, roots)) = &queries.batch else {
            return self.complete_one(&self.root, self.query, hashes, checked_runtime, None);
        };
        if self.query != *batch {
            return Err(operational("batch Run requires its batch root"));
        }
        let mut weight = crate::NativeMetadataWeight::default();
        weight.arc(&checked_runtime);
        for paths in [
            &checked_runtime.noalloc,
            &checked_runtime.noalloc_drop,
            &checked_runtime.flat,
        ] {
            weight.tree::<DefPath, ()>(paths.len());
            for path in paths {
                weight.path(path);
            }
        }
        weight.map(&checked_runtime.operations, |weight, path, _| {
            weight.path(path)
        });
        if let Some(path) = &checked_runtime.arena_with {
            weight.path(path);
        }
        weight.tree::<DefPath, Arc<FrontRunOutput>>(roots.len());
        weight.tree::<ContentHash, (ProductRef, Arc<FrontRunOutput>)>(roots.len());
        weight.add(std::mem::size_of::<FrontRunBatchOutput>() + 2 * std::mem::size_of::<usize>());
        let result = (|| {
            let mut products = Vec::with_capacity(roots.len());
            let mut outputs = BTreeMap::new();
            for root in roots {
                let entries = outputs
                    .values()
                    .find(|output: &&Arc<FrontRunOutput>| output.entry.0.package == root.0.package)
                    .map(|output| &output.entries);
                let product = self.complete_one(
                    root,
                    queries.ids[root],
                    hashes.clone(),
                    checked_runtime.clone(),
                    entries,
                )?;
                let output = queries
                    .outcome(product)
                    .ok_or_else(|| operational("batch entry output disappeared"))?;
                weight.path(root);
                weight.arc(&output);
                weight.path(&output.entry);
                weight.program_revision(&output.revision);
                weight.code_reach(&output.code);
                weight.foreign_use(&output.foreign);
                weight.callees(&output.callees);
                weight.map(&output.entries, |weight, name, (facts, _)| {
                    weight.add(name.capacity());
                    weight.code_reach(&facts.reach);
                });
                output.semantic.metadata_weight(&mut weight);
                if weight.bytes() > queries.weight.retained {
                    return Err(operational("Run batch outputs exceed retained admission"));
                }
                products.push((
                    product.address.0.0,
                    product.action.0.0,
                    product.record.domain.kind,
                    product.record.domain.schema,
                    product.record.output.0.0,
                    product.record.len,
                ));
                outputs.insert(root.clone(), output);
            }
            let bytes = queries
                .records
                .encode(&queries.anchors, &products)
                .ok_or_else(|| operational("Run batch metadata encoding failed"))?;
            let record = queries
                .store
                .put_content(BATCH_OUTPUT, &bytes)
                .ok_or(QueryError::NotImplemented)?;
            let action = hirsel_artifact_store::Action(ContentHash::of_parts(
                "hirsel.front.run.batch.action.v1",
                [batch.address.0.0.as_slice(), record.output.0.0.as_slice()],
            ));
            let product = ProductRef {
                address: batch.address,
                action,
                record,
            };
            queries
                .store
                .associate_product(product.address, product.action, product.record);
            *queries
                .completed_batch
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some((product, Arc::new(outputs)));
            Ok(product)
        })();
        if result.is_err() {
            let mut completed = queries
                .completed
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for root in roots {
                completed.remove(&queries.ids[root].address.0);
            }
        }
        result
    }
    fn complete_one(
        &self,
        root: &DefPath,
        query: QueryId,
        hashes: Arc<BTreeMap<DefPath, ContentHash>>,
        checked_runtime: Arc<hirsel_interp::checked::CheckedRuntime>,
        package_entries: Option<&BTreeMap<String, (CalleeFacts, CallForm)>>,
    ) -> Result<ProductRef, QueryError> {
        let queries = &self.queries;
        let snapshot = queries.lower.signatures();
        let unsafe_operations: BTreeSet<_> = hirsel_types::core_roles::intrinsic_roles()
            .into_iter()
            .filter(|(_, role)| *role == hirsel_types::core_roles::Intrinsic::UnsafeOperation)
            .map(|(path, _)| path)
            .collect();
        let escapes: BTreeMap<_, _> = self.entries.keys().map(|id| id.path()).filter_map(|path| {
            let signature = &queries.inputs.get(path)?.signature;
            if unsafe_operations.contains(path) { Some((path.clone(), Escape::UnsafeOperation)) }
            else if matches!(&signature.kind, SignatureKind::Function(f) if f.foreign.is_some()) { Some((path.clone(), Escape::Extern)) }
            else { None }
        }).collect();
        let reach = |root: &DefPath| -> Result<CodeReach, QueryError> {
            Ok(self
                .program(root)?
                .reach(root, &escapes)
                .map_or(CodeReach::Safe, |r| CodeReach::Escapes(r.spelled())))
        };
        let facts = |path: &DefPath| -> Result<Option<(CalleeFacts, CallForm)>, QueryError> {
            let signature = &queries
                .inputs
                .get(path)
                .ok_or_else(|| operational(format!("Run callable has no signature: {path}")))?
                .signature;
            let Ok(roots) = crate::admission::entry_roots(
                snapshot.types(),
                signature,
                hirsel_run_schema::LaunchKind::Call,
            ) else {
                return Ok(None);
            };
            let code = hashes.get(path).ok_or_else(|| {
                operational(format!("Run callable has no definition hash: {path}"))
            })?;
            let program = self.program(path)?;
            Ok(Some((
                CalleeFacts {
                    program: program.revision(path)?.digest(),
                    code: code.0,
                    fingerprint: signature.fingerprint.0,
                    reach: program
                        .reach(path, &escapes)
                        .map_or(CodeReach::Safe, |reach| CodeReach::Escapes(reach.spelled())),
                },
                if roots.is_empty() {
                    CallForm::Compute
                } else {
                    CallForm::Call
                },
            )))
        };
        let program = self.program(root)?;
        let mut callees = BTreeMap::new();
        for path in &program.definitions {
            let Some(content) = self
                .entries
                .get(&DefId::intern(path))
                .map(|entry| &entry.content)
                .filter(|content| content.has_body())
            else {
                continue;
            };
            let DefinitionContent::Body { tir: Some(tir), .. } =
                content.load(queries, path, queries.weight.scratch)?
            else {
                return Err(operational("Run content body disappeared"));
            };
            for site in hirsel_proof_export::call_sites(&tir, &hashes)
                .map_err(|p| operational(format!("call outside Run closure: {p}")))?
            {
                if matches!(site.kind, hirsel_proof_export::CallSiteKind::Reference(_))
                    && let Some((facts, _)) = facts(&site.callee)?
                {
                    callees.insert(site.callee.to_string(), facts);
                }
            }
        }
        let entries = if let Some(entries) = package_entries {
            entries.clone()
        } else {
            let mut entries = BTreeMap::new();
            for (path, input) in &queries.inputs {
                if input.source
                    && path.0.package == root.0.package
                    && let Some(facts) = facts(path)?
                {
                    entries.insert(path.to_string(), facts);
                }
            }
            entries
        };
        let foreign = foreign_use(&program, &queries.inputs);
        // PROG-4: C called outside a deterministic wrapper makes every run of
        // this entry unrecordable, as the checked package decides it.
        let unreplayable = !queries.check.unsafe_ffi().is_empty() && !foreign.unmodeled.is_empty();
        let revision = program.revision(root)?;
        let code = reach(root)?;
        let semantic_program = if queries.native_roots.is_empty() || queries.batch.is_some() {
            program.clone()
        } else {
            self.program_from(queries.native_roots.clone())?
        };
        let bindings = semantic_program
            .definitions
            .iter()
            .filter_map(|path| {
                let input = queries.inputs.get(path)?;
                input.source.then(|| {
                    (
                        path.clone(),
                        (input.implementation, input.signature.fingerprint),
                    )
                })
            })
            .collect();
        let semantic = Arc::new(crate::NativeSemanticProgram::new(
            semantic_program,
            escapes,
            queries.foreign_threads.clone(),
            snapshot.clone(),
            bindings,
            queries.foreign_links.clone(),
            unreplayable,
        ));
        let mut hash_bytes = crate::NativeMetadataWeight::default();
        let definition_hashes = if queries.export_hashes {
            hash_bytes.map(&hashes, |weight, path, _| weight.path(path));
            Some(hashes.clone())
        } else {
            None
        };
        let output = Arc::new(FrontRunOutput {
            definition_hashes,
            entry: root.clone(),
            revision,
            code,
            foreign,
            semantic,
            callees,
            entries,
            runtime: queries.runtime.clone(),
            checked_runtime,
            _owner: queries.ownership.clone(),
        });
        // Existing canonical launch encoders provide the compact published facts.
        let mut bytes = queries
            .records
            .encode(
                &queries.anchors,
                &(
                    output.entry.clone(),
                    output.revision.digest(),
                    hirsel_run_schema::callees_json(&output.callees),
                    output.entries.keys().collect::<Vec<_>>(),
                    output.foreign.externs.iter().collect::<Vec<_>>(),
                    output.foreign.unmodeled.iter().collect::<Vec<_>>(),
                ),
            )
            .ok_or_else(|| operational("Run output metadata encoding failed"))?;
        if let Some(hashes) = &output.definition_hashes {
            bytes = queries
                .records
                .encode(&queries.anchors, &(&bytes, &**hashes))
                .ok_or(QueryError::NotImplemented)?;
        }
        if (bytes.len() as u64).saturating_add(hash_bytes.bytes()) > queries.weight.retained {
            return Err(operational("Run output exceeds admission"));
        }
        let record = queries
            .store
            .put_content(OUTPUT, &bytes)
            .ok_or(QueryError::NotImplemented)?;
        let action = hirsel_artifact_store::Action(ContentHash::of_parts(
            "hirsel.front.run.action.v1",
            [query.address.0.0.as_slice(), record.output.0.0.as_slice()],
        ));
        let product = ProductRef {
            address: query.address,
            action,
            record,
        };
        queries
            .store
            .associate_product(product.address, product.action, product.record);
        queries
            .completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(query.address.0, (product, output));
        Ok(product)
    }
}
fn operational(message: impl Into<String>) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "admitted Run root".into(),
        message: message.into(),
    })
}
fn extend_reads(
    dependencies: &mut BTreeSet<DefPath>,
    reads: &Reads,
    mut lookup: impl FnMut(crate::memo::LookupKey) -> Option<DefPath>,
) {
    dependencies.extend(reads.iter().filter_map(|(read, _)| match read {
        ReadKey::Signature(id)
        | ReadKey::ExecutedSignature(id)
        | ReadKey::ExecutedBody(id)
        | ReadKey::LayoutBody(id)
        | ReadKey::Representation(id) => Some(id.path().clone()),
        ReadKey::Lookup(key) => lookup(*key),
        _ => None,
    }));
}
fn foreign_use(program: &SemanticProgram, inputs: &BTreeMap<DefPath, Input>) -> ForeignUse {
    let externs: BTreeSet<_> = program
        .definitions
        .iter()
        .filter(|path| {
            inputs.get(*path).is_some_and(
                |i| matches!(&i.signature.kind, SignatureKind::Function(f) if f.foreign.is_some()),
            )
        })
        .cloned()
        .collect();
    let mut unmodeled = BTreeMap::new();
    for (user, uses) in &program.uses {
        if !externs.contains(user)
            && let Some(called) = uses.iter().find(|p| externs.contains(*p))
            && !inputs.get(user).is_some_and(|i| {
                i.signature
                    .metadata
                    .wrapper
                    .is_some_and(|w| w.deterministic)
            })
        {
            unmodeled.insert(user.clone(), called.clone());
        }
    }
    ForeignUse { externs, unmodeled }
}
impl QueryEngine {
    pub fn front_run_queries(
        &mut self,
        check: Arc<FrontCheckQueries>,
        front: Arc<FrontBodyQueries>,
        lower: Arc<FrontLowerQueries>,
        weight: MemoryWeight,
        allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<FrontRunQueries>, QueryError> {
        if !Arc::ptr_eq(&lower.signatures(), &self.signature_inputs()) {
            return Err(operational(
                "Run and Lower require the same signature snapshot",
            ));
        }
        let inputs = self.front_run_inputs(allowance.bytes())?;
        self.front_run_queries_with_inputs(check, front, lower, inputs, weight, allowance, errors)
    }

    /// Capture before partitioning the existing host snapshot allowance. Both
    /// the planner and producer consume this one catalog's exact byte count.
    pub fn front_run_inputs(&mut self, limit: u64) -> Result<FrontRunInputs, QueryError> {
        let signatures = self.signature_inputs();
        let store = self
            .result_store
            .clone()
            .filter(|store| store.enabled())
            .ok_or(QueryError::NotImplemented)?;
        let declarations = self.frozen_declarations();
        let source_paths = self.source_paths();
        let mut paths: BTreeSet<_> = source_paths.iter().cloned().collect();
        paths.extend(self.native_builtin_signatures().into_keys());
        let mut inputs = BTreeMap::new();
        let mut views = Vec::new();
        let required = FrontRunQueries::input_bytes(&paths.iter().cloned().collect::<Vec<_>>())
            .ok_or_else(|| operational("Run input inventory overflow"))?;
        for path in paths {
            let source = source_paths.binary_search(&path).is_ok();
            let output = self.item_signature(&path)?;
            let signature = output.value;
            let owner_signature = signatures
                .body_view(&path.0.package, DefId::intern(&path))
                .ok_or(QueryError::NotImplemented)?
                .map_err(Clone::clone)?
                .clone();
            views.push((owner_signature.clone(), ContentHash::default()));
            let mut content = vec![
                hirsel_types::encoding::canonical_encoding(
                    &self.types,
                    &signature,
                    &output.dependencies,
                )
                .map_err(|e| operational(format!("signature encoding: {e:?}")))?,
            ];
            let mut dependencies: BTreeSet<_> = output
                .dependencies
                .iter()
                .map(|d| d.definition.path().clone())
                .collect();
            // Scalar semantic_body_node follows the complete Signature
            // observations too: normalized coverage can omit aliases and
            // primitive lookups whose spelling still belongs to PS9 closure.
            if let Some(reads) = self.shared_memo_reads(&path, QueryKind::Signature) {
                extend_reads(&mut dependencies, &reads, |key| {
                    FrontBodyQueries::lookup_in(&signatures, &declarations, key)
                });
            }
            if let Some(reads) = self.dependency_reads(&path, QueryKind::Signature) {
                dependencies.extend(reads.signatures.iter().map(|d| d.definition.path().clone()));
                for read in reads.resolver.iter().chain(&reads.manifest_lookups) {
                    match read {
                        hirsel_types::DependencyRead::Signature { definition, .. } => {
                            dependencies.insert(definition.path().clone());
                        }
                        hirsel_types::DependencyRead::Lookup {
                            result: Ok(definition),
                            ..
                        } => {
                            dependencies.insert(definition.clone());
                        }
                        hirsel_types::DependencyRead::ProviderLookup { output, .. } => {
                            if let Ok(definition) = &output.result {
                                dependencies.insert(definition.clone());
                            }
                        }
                        _ => {}
                    }
                }
            }
            let mut executable = false;
            let mut generated = false;
            let mut implementation = ContentHash::default();
            if source {
                self.materialize(&path)?;
                let syntax = self.snapshot(&path)?;
                executable = generation::executable(&syntax);
                generated = generation::generator(&syntax).is_some();
                implementation = syntax.text_hash;
                content.extend(self.semantic_tokens(&syntax)?);
            }
            if let SignatureKind::Type(declaration) = &signature.kind {
                dependencies.extend(declaration.drop.iter().cloned());
                if declaration.fact.is_some() {
                    dependencies.insert(hirsel_check_protocol::fact::fact_path(&path));
                }
                if let Some(check) = &declaration.serializable_check {
                    dependencies.insert(check.function.clone());
                }
            }
            if required > limit {
                return Err(operational("Run inputs exceed admission"));
            }
            let bytes = bincode::serialize(&content)
                .map_err(|_| operational("Run input content encoding"))?;
            let content = store
                .put_content(INPUT_CONTENT, &bytes)
                .filter(|reference| INPUT_CONTENT.reference(&bytes) == Some(*reference))
                .ok_or_else(|| operational("Run input content exceeds transport admission"))?;
            inputs.insert(
                path,
                Input {
                    signature,
                    owner_signature,
                    content,
                    dependencies,
                    source,
                    executable,
                    generated,
                    implementation,
                },
            );
        }
        Ok(FrontRunInputs {
            signatures,
            source_paths,
            inputs,
            views,
            store,
            required,
        })
    }

    pub fn front_run_queries_with_inputs(
        &mut self,
        check: Arc<FrontCheckQueries>,
        front: Arc<FrontBodyQueries>,
        lower: Arc<FrontLowerQueries>,
        inputs: FrontRunInputs,
        weight: MemoryWeight,
        allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<FrontRunQueries>, QueryError> {
        if !Arc::ptr_eq(&inputs.signatures, &self.signature_inputs())
            || !Arc::ptr_eq(&inputs.signatures, &lower.signatures())
        {
            return Err(operational(
                "Run inputs and Lower require the same signature snapshot",
            ));
        }
        if inputs.required > allowance.bytes() {
            return Err(operational("Run inputs exceed admission"));
        }
        let FrontRunInputs {
            signatures: _,
            source_paths,
            inputs,
            views,
            store,
            required,
        } = inputs;
        // Zero representation digests match ordinary execution. Compile-time
        // reflection's digest-bearing runtime remains a different producer.
        let runtime = Arc::new(self.build_runtime_types(views, false)?);
        let ids = source_paths
            .into_iter()
            .map(|path| {
                let id = QueryId {
                    kind: FRONT_RUN_KIND,
                    address: hirsel_artifact_store::Address(ContentHash::of_parts(
                        "hirsel.front.run.address.v1",
                        [
                            self.environment_value().0.as_slice(),
                            path.fingerprint().0.as_slice(),
                        ],
                    )),
                };
                (path, id)
            })
            .collect();
        Ok(Arc::new(FrontRunQueries {
            batch: None,
            completed_batch: Mutex::new(None),
            export_hashes: false,
            native_roots: Vec::new(),
            inputs,
            ids,
            check,
            front: front.clone(),
            lower: lower.clone(),
            runtime,
            foreign_threads: self.foreign_threads.clone(),
            foreign_links: self.foreign_links.clone(),
            records: crate::NativeRecords::new(self.types.clone(), store.clone()),
            anchors: crate::NativeRecordContext::capture(self),
            store,
            completed: Mutex::new(BTreeMap::new()),
            failures: Mutex::new(BTreeMap::new()),
            retention: Mutex::new(allowance.bytes() - required),
            weight,
            errors,
            ownership: Arc::new(Ownership {
                _front: front.clone(),
                _lower: lower.clone(),
                _allowance: allowance,
            }),
        }))
    }
}
