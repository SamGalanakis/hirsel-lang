//! Source GEN-5 over every required checked root. Native reachability never
//! selects this root set. Pending reads retain the frontier and bounded edges;
//! factories and output inspection execute no compiler phase.
use super::*;
use crate::runtime::*;
use crate::store::{Domain, Reference, ResultStore};
use std::{sync::Mutex, task::Poll};
pub const FRONT_GROWTH_KIND: u32 = 0x4600_0008;
const OUTPUT: Domain = Domain {
    kind: 0x4600_0009,
    schema: 1,
};
const FAILURE: Domain = Domain {
    kind: 0x4600_000c,
    schema: 1,
};
struct Data {
    growth: Arc<ParameterGrowth>,
    diagnostics: Arc<Vec<Diagnostic>>,
    reads: Arc<Reads>,
}
pub struct FrontGrowthQueries {
    query: QueryId,
    roots: Vec<QueryId>,
    front: Arc<FrontBodyQueries>,
    items: Arc<crate::NativeRecordContext>,
    records: crate::NativeRecords,
    sources: SourceMap,
    snapshot: Arc<SignatureSnapshot>,
    package: hirsel_syntax::PackageId,
    store: Arc<dyn ResultStore>,
    completed: Mutex<Option<(ProductRef, Arc<Data>)>>,
    failure: Mutex<Option<(Reference, QueryError)>>,
    weight: MemoryWeight,
    errors: RuntimeErrors,
    output_budget: u64,
    _allowance: MemoryReservation,
}
#[derive(Clone)]
pub struct FrontGrowthOutput {
    pub growth: Arc<ParameterGrowth>,
    pub diagnostics: Arc<Vec<Diagnostic>>,
    pub reads: Arc<Reads>,
    _owner: Arc<FrontGrowthQueries>,
}
impl FrontGrowthQueries {
    /// Root handles and the owner itself. The exact front keeps source paths,
    /// anchors and body outputs; the admitted task keeps its execution frame.
    pub fn input_bytes(definitions: &[DefPath], _weight: MemoryWeight) -> Option<u64> {
        let package = definitions
            .iter()
            .map(|path| path.0.package.name.len() + path.0.package.version.len())
            .max()
            .unwrap_or(0);
        (std::mem::size_of::<Self>() as u64)
            .checked_add((2 * std::mem::size_of::<usize>()) as u64)?
            .checked_add(package as u64)?
            .checked_add(
                (definitions.len() as u64).checked_mul(std::mem::size_of::<QueryId>() as u64)?,
            )
    }
    pub fn failure(&self, reference: Reference) -> Option<QueryError> {
        self.failure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|(selected, error)| (*selected == reference).then(|| error.clone()))
    }
    pub fn query(&self) -> QueryId {
        self.query
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    pub fn outcome(self: &Arc<Self>, product: ProductRef) -> Option<FrontGrowthOutput> {
        let completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        let (selected, data) = completed.as_ref()?;
        (*selected == product).then(|| FrontGrowthOutput {
            growth: data.growth.clone(),
            diagnostics: data.diagnostics.clone(),
            reads: data.reads.clone(),
            _owner: self.clone(),
        })
    }
}
struct Factory(Arc<FrontGrowthQueries>);
impl QueryFactory for Factory {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if id != self.0.query {
            return Err(self.0.errors.protocol);
        }
        Ok(Box::new(Task {
            queries: self.0.clone(),
            selected: vec![None; self.0.roots.len()],
            cursor: 0,
            edges: Vec::new(),
            table_keys: BTreeMap::new(),
            required: BTreeMap::new(),
            reads: ReadsBuilder::new(),
            terminal: None,
        }))
    }
}
struct Task {
    queries: Arc<FrontGrowthQueries>,
    selected: Vec<Option<Result<ProductRef, Reference>>>,
    cursor: usize,
    edges: Vec<crate::GrowthEdge>,
    table_keys: BTreeMap<DefPath, Arc<crate::table_keys::TableKeyUse>>,
    required: BTreeMap<DefPath, BTreeSet<usize>>,
    reads: ReadsBuilder,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.queries.query
    }
    fn weight(&self) -> MemoryWeight {
        self.queries.weight
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if let Some(result) = self.terminal {
            return finish(result);
        }
        match self.advance(cx) {
            Poll::Pending => QueryPoll::Waiting(
                self.queries
                    .roots
                    .iter()
                    .enumerate()
                    .filter_map(|(i, id)| self.selected[i].is_none().then_some(*id))
                    .collect(),
            ),
            Poll::Ready(result) => {
                self.terminal = Some(result);
                finish(result)
            }
        }
    }
}
fn finish(result: Result<ProductRef, Reference>) -> QueryPoll {
    match result {
        Ok(product) => QueryPoll::Complete(product),
        Err(error) => QueryPoll::Failed(error),
    }
}
impl Task {
    fn advance(&mut self, cx: &mut dyn QueryContext) -> Poll<Result<ProductRef, Reference>> {
        for (i, query) in self.queries.roots.iter().enumerate() {
            if self.selected[i].is_none()
                && let Poll::Ready(result) = cx.read(*query)
            {
                self.selected[i] = Some(result);
            }
        }
        while let Some(Some(selected)) = self.selected.get(self.cursor).copied() {
            let reference = selected.map(|p| p.record).unwrap_or_else(|r| r);
            let Some(checked) = self.queries.front.checked(reference) else {
                return Poll::Ready(Err(selected.err().unwrap_or(self.queries.errors.protocol)));
            };
            for call in &checked.summary.summary.calls {
                let slots = hirsel_types::core_roles::table_key_parameters(call.callee.path());
                if !slots.is_empty() {
                    self.required
                        .entry(call.callee.path().clone())
                        .or_default()
                        .extend(slots);
                }
            }
            self.table_keys.insert(
                checked.summary.summary.owner.path().clone(),
                checked.table_keys,
            );
            self.reads.merge(&checked.reads);
            self.edges
                .extend(checked.summary.growth.edges.iter().cloned());
            if (self.edges.len() as u64)
                .saturating_mul(std::mem::size_of::<crate::GrowthEdge>() as u64)
                > self.queries.output_budget
            {
                return Poll::Ready(Err(self.queries.errors.resource));
            }
            self.cursor += 1;
        }
        if self.cursor < self.selected.len() {
            return Poll::Pending;
        }
        Poll::Ready(self.complete())
    }
    fn complete(&mut self) -> Result<ProductRef, Reference> {
        let queries = &self.queries;
        let mut table_reads = ReadsBuilder::new();
        let mut provider =
            FrozenProvider::new(&queries.snapshot, queries.package.clone(), &mut table_reads);
        let table_result = crate::table_keys::check_uses(
            queries.snapshot.types(),
            &self.table_keys,
            std::mem::take(&mut self.required),
            &mut |definition| provider.signature(definition).map(Some),
        );
        self.reads
            .merge(&table_reads.finish().map_err(|_| queries.errors.protocol)?);
        let reads = std::mem::take(&mut self.reads)
            .finish()
            .map_err(|_| queries.errors.protocol)?;
        let table_diagnostics = match table_result {
            Ok(diagnostics) => diagnostics,
            Err(error) => {
                let bytes = queries
                    .records
                    .encode(&*queries.items, &(&error, &reads))
                    .ok_or(queries.errors.resource)?;
                if bytes.len() as u64 > queries.weight.scratch {
                    return Err(queries.errors.resource);
                }
                let reference = queries
                    .store
                    .put_content(FAILURE, &bytes)
                    .ok_or(queries.errors.resource)?;
                *queries.failure.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some((reference, error));
                return Err(reference);
            }
        };
        let growth = ParameterGrowth {
            edges: std::mem::take(&mut self.edges).into(),
        };
        let diagnostics = crate::select_diagnostics(
            table_diagnostics
                .into_iter()
                .chain(crate::growing_sites(&growth.edges).into_iter().map(|site| {
                    Diagnostic::error(
                        const { RuleId::of("GEN-5") },
                        site,
                        "a recursive instantiation cycle grows its type arguments",
                    )
                }))
                .collect(),
            Some(&queries.sources),
        );
        let bytes = queries
            .records
            .encode(&*queries.items, &(&growth, &diagnostics, &reads))
            .ok_or(queries.errors.resource)?;
        if bytes.len() as u64 > queries.weight.scratch {
            return Err(queries.errors.resource);
        }
        let data = Arc::new(Data {
            growth: Arc::new(growth),
            diagnostics: Arc::new(diagnostics),
            reads: queries
                .front
                .retained_reads(&Arc::new(reads))
                .ok_or(queries.errors.resource)?,
        });
        // Encoded bytes are temporary scratch and belong to the result store
        // after publication. Bound only the actual retained allocation graph.
        let mut retained = crate::memo::retained::Weight::default();
        retained.arc(
            &data,
            |w,
             Data {
                 growth,
                 diagnostics,
                 reads,
             }| {
                w.arc(growth, |w, growth| {
                    w.add(std::mem::size_of_val(&*growth.edges))
                });
                w.arc(diagnostics, |w, diagnostics| {
                    w.vec(diagnostics, |w, diagnostic| w.diagnostic(diagnostic))
                });
                w.reads(reads);
            },
        );
        if retained.bytes > queries.output_budget {
            return Err(queries.errors.resource);
        }
        let record = queries
            .store
            .put_content(OUTPUT, &bytes)
            .ok_or(queries.errors.resource)?;
        // Canonical required member outputs, not completion order or type IDs,
        // identify the actual source-growth computation.
        let mut members = Vec::new();
        for selected in self.selected.iter().flatten() {
            match selected {
                Ok(product) => members.extend(product.record.output.0.0),
                Err(reference) => members.extend(reference.output.0.0),
            }
        }
        let action = hirsel_artifact_store::Action(ContentHash::of_parts(
            "hirsel.front.source-growth.action.v1",
            [queries.query.address.0.0.as_slice(), members.as_slice()],
        ));
        let product = ProductRef {
            address: queries.query.address,
            action,
            record,
        };
        *queries.completed.lock().unwrap_or_else(|e| e.into_inner()) = Some((product, data));
        queries
            .store
            .associate_product(product.address, product.action, product.record);
        Ok(product)
    }
}
impl QueryEngine {
    /// Settle headers and capture the existing source-language root set. This
    /// includes unexecuted declarations and excludes registry-only bodies.
    /// It performs no body checking, lowering or compile-time execution.
    pub fn front_source_roots(&mut self) -> Vec<DefPath> {
        let _ = self.signature_inputs();
        // The source-only index restores every declaration but no legacy
        // growth controller. Its pending set therefore contains only edited
        // declarations, never the complete source Check obligation set.
        let mut roots: BTreeSet<_> = self.source_paths().into_iter().collect();
        // A warm source catalog carries only the written declaration. Its fact
        // helper is still a required body of the source check transaction.
        for path in roots.clone() {
            if self.source_facts(&path).is_some_and(|facts| facts.fact) {
                let helper = hirsel_check_protocol::fact::fact_path(&path);
                let _ = self.materialize(&path);
                let _ = self.item_signature(&helper);
                roots.insert(helper);
            }
        }
        roots.into_iter().collect()
    }
    /// Capture the complete selected source-root set, including declarations
    /// never executed by the launch entry. A partial front view is refused.
    pub fn front_growth_queries(
        &mut self,
        root: &DefPath,
        front: Arc<FrontBodyQueries>,
        weight: MemoryWeight,
        allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<FrontGrowthQueries>, QueryError> {
        let refusal = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "source growth admission".into(),
                message: "required source roots are missing or exceed admission".into(),
            })
        };
        let definitions = self.front_source_roots();
        if FrontGrowthQueries::input_bytes(&definitions, weight)
            .is_none_or(|n| n > allowance.bytes())
        {
            return Err(refusal());
        }
        // Keep this backing exact so the forecast and retained-input debit
        // describe the same allocation even for non-power-of-two root counts.
        let mut roots = Vec::with_capacity(definitions.len());
        for path in &definitions {
            roots.push(front.query(path).ok_or_else(|| {
                QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "source growth admission".into(),
                    message: format!("missing checked-root producer for {path:?}"),
                })
            })?);
        }
        // The settled owner retains only root IDs and handles to the exact
        // front source. Charge its real backing before assigning the remainder
        // of this same reservation to its completed output.
        let source = front.stage_source();
        let package = root.0.package.clone();
        let mut retained = crate::NativeMetadataWeight::default();
        retained.add(std::mem::size_of::<FrontGrowthQueries>() + 2 * std::mem::size_of::<usize>());
        retained.flat_vec(&roots);
        retained.add(package.name.capacity() + package.version.capacity());
        let output_budget = allowance
            .bytes()
            .checked_sub(retained.bytes())
            .ok_or_else(refusal)?;
        let query = QueryId {
            kind: FRONT_GROWTH_KIND,
            address: hirsel_artifact_store::Address(ContentHash::of_parts(
                "hirsel.front.source-growth.address.v1",
                [
                    self.environment_value().0.as_slice(),
                    root.fingerprint().0.as_slice(),
                ],
            )),
        };
        let store = self
            .result_store
            .clone()
            .filter(|s| s.enabled())
            .ok_or_else(refusal)?;
        Ok(Arc::new(FrontGrowthQueries {
            query,
            roots,
            front,
            // The front owner remains held here, including its original
            // source/context reservation. No copied anchor/path tables.
            items: source.anchors,
            records: source.records,
            sources: self.sources.clone(),
            snapshot: source.snapshot,
            package,
            store,
            completed: Mutex::new(None),
            failure: Mutex::new(None),
            weight,
            errors,
            output_budget,
            _allowance: allowance,
        }))
    }
}
