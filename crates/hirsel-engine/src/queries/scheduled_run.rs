//! Immutable launch facts from the admitted source semantic closure.
use super::*;
mod proof_export;
use hirsel_run_schema::{CallForm, CalleeFacts, CodeReach, ProgramRevision};
pub use proof_export::{
    FrontBuiltProof, FrontPreparedProof, FrontProofExport, FrontProofInput, FrontProofText,
};
/// Metadata consumed by interpreter/H/native hosts. No mutable query engine,
/// body cache or native Value reconstruction is available through this output.
pub struct FrontRunOutput {
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
            .unwrap_or_else(|e| e.into_inner())
            .get(&query.address.0)
            .map(|(product, _)| *product);
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
            nodes: BTreeMap::new(),
            contents: BTreeMap::new(),
            uses: BTreeMap::new(),
            waiting: None,
            terminal: completed.map(Ok),
        }))
    }
}
struct Task {
    queries: Arc<FrontRunQueries>,
    query: QueryId,
    root: DefPath,
    checked: bool,
    pending: Vec<DefPath>,
    nodes: BTreeMap<DefPath, hirsel_run_schema::ProgramNode>,
    contents: BTreeMap<DefPath, DefinitionContent>,
    uses: BTreeMap<DefPath, BTreeSet<DefPath>>,
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
                return Poll::Ready(Err(self.queries.failure_record(QueryError::NotImplemented)));
            }
            if has_errors(&output.checked.diagnostics) {
                return Poll::Ready(Err(self
                    .queries
                    .failure_record(QueryError::Diagnostics(output.checked.diagnostics))));
            }
            self.checked = true;
            self.pending.push(self.root.clone());
            self.pending
                .extend(self.queries.native_roots.iter().cloned());
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
                    self.pending.push(path.clone());
                }
            }
            // Runtime type construction historically demands all declared drop
            // bodies. These are explicit S5 dependencies, never host work.
            for nominal in &self.queries.runtime.nominals {
                if let Some((path, _)) = &nominal.finalizer {
                    self.pending.push(path.clone());
                }
            }
        }
        while let Some(path) = self.pending.last().cloned() {
            if self.nodes.contains_key(&path) {
                self.pending.pop();
                continue;
            }
            match self.node(cx, &path) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok((node, content, dependencies))) => {
                    self.pending.pop();
                    self.pending.extend(
                        dependencies
                            .iter()
                            .filter(|p| !self.nodes.contains_key(*p))
                            .cloned(),
                    );
                    let semantic = node.dependencies.iter().collect::<BTreeSet<_>>();
                    let semantic_dependencies = dependencies
                        .into_iter()
                        .filter(|p| semantic.contains(&p.to_string()))
                        .collect();
                    self.nodes.insert(path.clone(), node);
                    self.contents.insert(path.clone(), content);
                    self.uses.insert(path, semantic_dependencies);
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
        Poll::Ready(
            self.complete()
                .map_err(|error| self.queries.failure_record(error)),
        )
    }
    fn node(
        &mut self,
        cx: &mut dyn QueryContext,
        path: &DefPath,
    ) -> Poll<
        Result<
            (
                hirsel_run_schema::ProgramNode,
                DefinitionContent,
                BTreeSet<DefPath>,
            ),
            Reference,
        >,
    > {
        let queries = self.queries.clone();
        let Some(input) = queries.inputs.get(path) else {
            return Poll::Ready(Err(queries.failure_record(operational(format!(
                "missing signature input for {path}"
            )))));
        };
        let mut dependencies = input.dependencies.clone();
        let mut tir = None;
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
        let source_content = match source_content {
            Ok(content) => content,
            Err(error) => return Poll::Ready(Err(queries.failure_record(error))),
        };
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
        let mut spelled: Vec<_> = dependencies.iter().map(ToString::to_string).collect();
        spelled.push("$hirsel/runtime@1".into());
        Poll::Ready(Ok((
            hirsel_run_schema::ProgramNode {
                identity: path.to_string(),
                content: source_content,
                dependencies: spelled,
            },
            content,
            all_dependencies,
        )))
    }
    fn graph_bytes(&self) -> Option<u64> {
        let bodies = self.contents.values().try_fold(0u64, |n, content| {
            let bytes = match content {
                DefinitionContent::Body { tir: Some(tir), .. } => {
                    hirsel_types::with_table_collection(|| bincode::serialized_size(tir))
                        .0
                        .ok()?
                }
                _ => 0,
            };
            n.checked_add(bytes)
        })?;
        self.nodes.values().try_fold(bodies, |n, node| {
            let n = n
                .checked_add(node.identity.len() as u64)?
                .checked_add(256)?;
            let n = node
                .content
                .iter()
                .try_fold(n, |n, c| n.checked_add(c.len() as u64))?;
            node.dependencies
                .iter()
                .try_fold(n, |n, d| n.checked_add(d.len() as u64))
        })
    }
    fn program(&self, root: &DefPath) -> SemanticProgram {
        self.program_from(vec![root.clone()])
    }
    fn program_from(&self, mut pending: Vec<DefPath>) -> SemanticProgram {
        let mut definitions = BTreeSet::new();
        while let Some(path) = pending.pop() {
            if definitions.insert(path.clone()) {
                pending.extend(self.uses.get(&path).into_iter().flatten().cloned());
            }
        }
        let mut nodes = vec![super::program_revision::runtime_node()];
        nodes.extend(
            definitions
                .iter()
                .filter_map(|p| self.nodes.get(p).cloned()),
        );
        SemanticProgram {
            nodes,
            uses: definitions
                .iter()
                .filter_map(|p| self.uses.get(p).map(|uses| (p.clone(), uses.clone())))
                .collect(),
            definitions,
        }
    }
    fn complete(&self) -> Result<ProductRef, QueryError> {
        let queries = &self.queries;
        let snapshot = queries.lower.signatures();
        let hashes = hirsel_proof_export::definition_hashes(snapshot.types(), &self.contents)
            .map_err(operational)?;
        let unsafe_operations: BTreeSet<_> = hirsel_types::core_roles::intrinsic_roles()
            .into_iter()
            .filter(|(_, role)| *role == hirsel_types::core_roles::Intrinsic::UnsafeOperation)
            .map(|(path, _)| path)
            .collect();
        let escapes: BTreeMap<_, _> = self.nodes.keys().filter_map(|path| {
            let signature = &queries.inputs.get(path)?.signature;
            if unsafe_operations.contains(path) { Some((path.clone(), Escape::UnsafeOperation)) }
            else if matches!(&signature.kind, SignatureKind::Function(f) if f.foreign.is_some()) { Some((path.clone(), Escape::Extern)) }
            else { None }
        }).collect();
        let reach = |root: &DefPath| {
            self.program(root)
                .reach(root, &escapes)
                .map_or(CodeReach::Safe, |r| CodeReach::Escapes(r.spelled()))
        };
        let facts = |path: &DefPath| -> Result<Option<(CalleeFacts, CallForm)>, QueryError> {
            let signature = &queries
                .inputs
                .get(path)
                .ok_or(QueryError::NotImplemented)?
                .signature;
            let Ok(roots) = crate::admission::entry_roots(
                snapshot.types(),
                signature,
                hirsel_run_schema::LaunchKind::Call,
            ) else {
                return Ok(None);
            };
            let code = hashes.get(path).ok_or(QueryError::NotImplemented)?;
            Ok(Some((
                CalleeFacts {
                    program: self.program(path).revision(path)?.digest(),
                    code: code.0,
                    fingerprint: signature.fingerprint.0,
                    reach: reach(path),
                },
                if roots.is_empty() {
                    CallForm::Compute
                } else {
                    CallForm::Call
                },
            )))
        };
        let program = self.program(&self.root);
        let mut callees = BTreeMap::new();
        for path in &program.definitions {
            let Some(DefinitionContent::Body { tir: Some(tir), .. }) = self.contents.get(path)
            else {
                continue;
            };
            for site in hirsel_proof_export::call_sites(tir, &hashes)
                .map_err(|p| operational(format!("call outside Run closure: {p}")))?
            {
                if matches!(site.kind, hirsel_proof_export::CallSiteKind::Reference(_))
                    && let Some((facts, _)) = facts(&site.callee)?
                {
                    callees.insert(site.callee.to_string(), facts);
                }
            }
        }
        let mut entries = BTreeMap::new();
        for (path, input) in &queries.inputs {
            if input.source
                && path.0.package == self.root.0.package
                && let Some(facts) = facts(path)?
            {
                entries.insert(path.to_string(), facts);
            }
        }
        let foreign = foreign_use(&program, &queries.inputs);
        // PROG-4: C called outside a deterministic wrapper makes every run of
        // this entry unrecordable, as the checked package decides it.
        let unreplayable = !queries.check.unsafe_ffi().is_empty() && !foreign.unmodeled.is_empty();
        let revision = program.revision(&self.root)?;
        let code = reach(&self.root);
        let semantic_program = if queries.native_roots.is_empty() {
            program.clone()
        } else {
            self.program_from(queries.native_roots.clone())
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
        let checked_runtime = Arc::new(snapshot.checked_runtime(queries.runtime.forget.as_ref())?);
        let output = Arc::new(FrontRunOutput {
            entry: self.root.clone(),
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
        let bytes = queries
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
            .ok_or(QueryError::NotImplemented)?;
        if bytes.len() as u64 > queries.weight.retained {
            return Err(operational("Run output exceeds admission"));
        }
        let record = queries
            .store
            .put_content(OUTPUT, &bytes)
            .ok_or(QueryError::NotImplemented)?;
        let action = hirsel_artifact_store::Action(ContentHash::of_parts(
            "hirsel.front.run.action.v1",
            [
                self.query.address.0.0.as_slice(),
                record.output.0.0.as_slice(),
            ],
        ));
        let product = ProductRef {
            address: self.query.address,
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
            .insert(self.query.address.0, (product, output));
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
            let mut implementation = ContentHash::default();
            if source {
                self.materialize(&path)?;
                let syntax = self.snapshot(&path)?;
                executable = generation::executable(&syntax);
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
