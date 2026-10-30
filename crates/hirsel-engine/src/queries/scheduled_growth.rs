//! Source GEN-5 over every required checked root. Native reachability never
//! selects this root set. Pending reads retain the frontier and compact facts;
//! factories and output inspection execute no compiler phase.
use super::*;
use crate::runtime::*;
use crate::store::{Domain, Reference, ResultStore};
use std::{sync::Mutex, task::Poll};
pub const FRONT_GROWTH_KIND: u32 = 0x4600_0008;
const OUTPUT: Domain = Domain {
    kind: 0x4600_0009,
    schema: 3,
};
const OBSERVATIONS: Domain = Domain {
    kind: 0x4600_0023,
    schema: 1,
};
const FAILURE: Domain = Domain {
    kind: 0x4600_000c,
    schema: 1,
};
struct Data {
    facets: Arc<SourceGrowthFacets>,
    diagnostics: Arc<Vec<Diagnostic>>,
    reads: Arc<Reads>,
    _reservation: Option<MemoryReservation>,
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
    archive_pool: MemoryReservationPool,
    _allowance: MemoryReservation,
}
#[derive(Clone)]
pub struct FrontGrowthOutput {
    pub facets: Arc<SourceGrowthFacets>,
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
    fn producer(&self) -> SourceGrowthProducer<'_> {
        SourceGrowthProducer {
            records: &self.records,
            source: &self.items,
            store: &*self.store,
            archive_pool: &self.archive_pool,
            namespace: self.query.address,
            scratch: self.weight.scratch,
        }
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
            facets: data.facets.clone(),
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
        let fact_bytes = (self.0.roots.len() as u64)
            .saturating_mul(std::mem::size_of::<SourceGrowthFact>() as u64);
        if fact_bytes > self.0.output_budget {
            return Err(self.0.errors.resource);
        }
        Ok(Box::new(Task {
            queries: self.0.clone(),
            selected: vec![None; self.0.roots.len()],
            cursor: 0,
            facts: Vec::with_capacity(self.0.roots.len()),
            fact_bytes,
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
    facts: Vec<SourceGrowthFact>,
    fact_bytes: u64,
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
        if let Some(result) = self.terminal.clone() {
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
                self.terminal = Some(result.clone());
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
        while let Some(Some(selected)) = self.selected.get(self.cursor).cloned() {
            let Some(checked) = self.queries.front.checked(selected.clone()) else {
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
            let observed = selected
                .as_ref()
                .map(|product| product.action.0)
                .unwrap_or_else(|reference| reference.output.0);
            let Some(fact) = self.queries.producer().fact_observed(
                self.queries.roots[self.cursor].address,
                checked.summary,
                selected.into(),
                observed,
            ) else {
                return Poll::Ready(Err(self.queries.errors.resource));
            };
            self.fact_bytes = self.fact_bytes.saturating_add(
                fact.input_bytes() - std::mem::size_of::<SourceGrowthFact>() as u64,
            );
            if self.fact_bytes > self.queries.output_budget {
                return Poll::Ready(Err(self.queries.errors.resource));
            }
            self.facts.push(fact);
            self.cursor += 1;
        }
        if self.cursor < self.selected.len() {
            return Poll::Pending;
        }
        Poll::Ready(self.complete(cx))
    }
    fn complete(&mut self, cx: &mut dyn QueryContext) -> Result<ProductRef, Reference> {
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
        let facts = std::mem::take(&mut self.facts);
        let fact_reservation = queries
            .archive_pool
            .take(self.fact_bytes)
            .ok_or(queries.errors.resource)?;
        let facets = queries
            .producer()
            .settle(&facts)
            .ok_or(queries.errors.resource)?;
        drop(facts);
        drop(fact_reservation);
        let diagnostics = crate::select_diagnostics(
            table_diagnostics
                .into_iter()
                .chain(
                    facets
                        .components
                        .iter()
                        .flat_map(|component| component.growing.iter().copied())
                        .map(|site| {
                            Diagnostic::error(
                                const { RuleId::of("GEN-5") },
                                site,
                                "a recursive instantiation cycle grows its type arguments",
                            )
                        }),
                )
                .collect(),
            Some(&queries.sources),
        );
        let outputs: Vec<_> = facets
            .components
            .iter()
            .map(|component| {
                component
                    .product
                    .archive(ArchiveAdmission::Retained)
                    .map(|archive| archive.reference())
            })
            .collect::<Result<_, _>>()
            .map_err(|_| queries.errors.resource)?;
        let membership = facets
            .membership
            .archive(ArchiveAdmission::Retained)
            .map_err(|_| queries.errors.resource)?
            .reference();
        // Like Lower's descriptor/body records, retain one small descriptor
        // and publish its complete observation rows as referenced content.
        // The live read view independently retains its source admission.
        let observed = publish_observations(
            &queries.records,
            &queries.items,
            &*queries.store,
            &reads,
            queries.weight.scratch,
        )
        .ok_or(queries.errors.resource)?;
        let bytes = queries
            .records
            .encode(
                &*queries.items,
                &(membership, &outputs, &diagnostics, observed),
            )
            .ok_or(queries.errors.resource)?;
        if bytes.len() as u64 > queries.weight.scratch {
            return Err(queries.errors.resource);
        }
        let mut data = Arc::new(Data {
            facets: Arc::new(facets),
            diagnostics: Arc::new(diagnostics),
            reads: queries
                .front
                .retained_reads(cx, &Arc::new(reads))
                .ok_or(queries.errors.resource)?,
            _reservation: None,
        });
        // Encoded bytes are temporary scratch and belong to the result store
        // after publication. Bound only the actual retained allocation graph.
        let mut retained = crate::memo::retained::Weight::default();
        retained.arc(
            &data,
            |w,
             Data {
                 facets,
                 diagnostics,
                 reads,
                 _reservation: _,
             }| {
                w.arc(facets, |w, facets| {
                    w.vec(&facets.components, |w, component| {
                        w.add(std::mem::size_of_val(&*component.members));
                        w.add(std::mem::size_of_val(&*component.growing));
                    });
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
        Arc::get_mut(&mut data)
            .ok_or(queries.errors.protocol)?
            ._reservation = Some(
            queries
                .archive_pool
                .take(retained.bytes)
                .ok_or(queries.errors.resource)?,
        );
        let record = queries
            .store
            .put_content(OUTPUT, &bytes)
            .ok_or(queries.errors.resource)?;
        // Downstream growth observes settled facets, not every checked body.
        // Current source observations remain mandatory transaction evidence.
        let action = hirsel_artifact_store::Action(ContentHash::of_parts(
            "hirsel.front.source-growth.action.v2",
            [queries.query.address.0.0.as_slice(), bytes.as_slice()],
        ));
        let product = ProductRef::import_archive(
            queries.query.address,
            action,
            ProductArchive::from_encoded_in(record, bytes, &queries.archive_pool)
                .ok()
                .ok_or(queries.errors.resource)?,
        );
        *queries.completed.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((product.clone(), data));
        queries
            .store
            .associate_product(product.address, product.action, record);
        Ok(product)
    }
}
impl QueryEngine {
    /// Enumerate required source declarations from the index, including
    /// unexecuted declarations and fact helpers, without demanding headers.
    pub fn front_source_roots(&mut self) -> Vec<DefPath> {
        // The source-only index restores every declaration but no legacy
        // growth controller. Its pending set therefore contains only edited
        // declarations, never the complete source Check obligation set.
        let mut roots: BTreeSet<_> = self.source_paths().into_iter().collect();
        // A warm source catalog carries only the written declaration. Its fact
        // helper is still a required body of the source check transaction.
        for path in roots.clone() {
            if self.source_facts(&path).is_some_and(|facts| facts.fact) {
                let helper = hirsel_check_protocol::fact::fact_path(&path);
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
        mut allowance: MemoryReservation,
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
        let output = allowance.split(output_budget).ok_or_else(refusal)?;
        let archive_pool = MemoryReservationPool::new(output).map_err(|_| refusal())?;
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
            archive_pool,
            _allowance: allowance,
        }))
    }
}

pub(super) fn publish_observations(
    records: &crate::NativeRecords,
    source: &crate::NativeRecordContext,
    store: &dyn ResultStore,
    reads: &Reads,
    scratch: u64,
) -> Option<Reference> {
    let bytes = records.encode(source, reads)?;
    (bytes.len() as u64 <= scratch)
        .then(|| store.put_content(OBSERVATIONS, &bytes))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_record_preserves_every_row_and_refuses_short_working_space() {
        let engine = QueryEngine::new(crate::default_environment(), Default::default());
        let source = crate::NativeRecordContext::capture(&engine);
        let store = Arc::new(crate::store::MemStore::default());
        let records = crate::NativeRecords::new(engine.types().clone(), store.clone());
        let reads = Reads::from_vec(
            (0u32..1024)
                .map(|i| {
                    (
                        ReadKey::ExecutedBody(DefId::intern(&DefPath::core(&format!(
                            "observed_{i}"
                        )))),
                        ContentHash::of_bytes(&i.to_le_bytes()),
                    )
                })
                .collect(),
        )
        .unwrap();
        let bytes = records.encode(&source, &reads).unwrap();
        let reference = OBSERVATIONS.reference(&bytes).unwrap();
        assert!(
            publish_observations(&records, &source, &*store, &reads, bytes.len() as u64 - 1)
                .is_none()
        );
        assert!(store.get_content(reference).is_none());
        assert_eq!(
            publish_observations(&records, &source, &*store, &reads, bytes.len() as u64),
            Some(reference)
        );
        let restored: Reads = records
            .decode(&source, &store.get_content(reference).unwrap())
            .unwrap();
        assert_eq!(restored, reads);
        assert_eq!(restored.len(), 1024);
    }
}
