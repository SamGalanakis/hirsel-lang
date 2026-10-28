//! Source storage closure over actual settled checked and recipe products.
//! Port of storage_graph::derive: backward callers select the frontier, then
//! actual callee/constant references extend it. The solver is shared with the
//! scalar adapter and is never used as a substitute for V's independent proof.
use super::*;
use crate::runtime::*;
use crate::store::{Domain, Reference, ResultStore};
use hirsel_types::storage::{CallableStorageProfile, graph::*};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    task::Poll,
};
pub const FRONT_STORAGE_GRAPH_KIND: u32 = 0x4600_0014;
const OUTPUT: Domain = Domain {
    kind: 0x4600_0015,
    schema: 1,
};
const FAILURE: Domain = Domain {
    kind: 0x4600_0016,
    schema: 1,
};
struct Data {
    profiles: Arc<Vec<CallableStorageProfile>>,
    diagnostics: Arc<Vec<Diagnostic>>,
    reads: Arc<Reads>,
}
pub struct FrontStorageGraphOutput {
    pub profiles: Arc<Vec<CallableStorageProfile>>,
    pub diagnostics: Arc<Vec<Diagnostic>>,
    pub reads: Arc<Reads>,
    _owner: Arc<FrontStorageGraphQueries>,
}
pub struct FrontStorageGraphQueries {
    query: QueryId,
    definitions: Vec<DefPath>,
    instances: BTreeSet<DefPath>,
    front: Arc<FrontBodyQueries>,
    growth: Arc<FrontGrowthQueries>,
    recipes: Arc<FrontStorageQueries>,
    lower: Arc<FrontLowerQueries>,
    records: crate::NativeRecords,
    anchors: crate::NativeRecordContext,
    store: Arc<dyn ResultStore>,
    completed: Mutex<Option<(ProductRef, Arc<Data>)>>,
    failure: Mutex<Option<(Reference, QueryError)>>,
    executions: AtomicU64,
    weight: MemoryWeight,
    errors: RuntimeErrors,
    output_budget: u64,
    _allowance: MemoryReservation,
}
impl FrontStorageGraphQueries {
    pub(crate) fn matches(
        &self,
        front: &Arc<FrontBodyQueries>,
        lower: &Arc<FrontLowerQueries>,
        growth: &Arc<FrontGrowthQueries>,
    ) -> bool {
        Arc::ptr_eq(&self.front, front)
            && Arc::ptr_eq(&self.lower, lower)
            && Arc::ptr_eq(&self.growth, growth)
    }
    pub fn input_bytes(definitions: &[DefPath]) -> Option<u64> {
        definitions
            .iter()
            .try_fold(std::mem::size_of::<Self>() as u64, |n, path| {
                n.checked_add(bincode::serialized_size(path).ok()?.checked_mul(3)?)?
                    .checked_add(
                        (std::mem::size_of::<DefPath>()
                            + std::mem::size_of::<DefId>()
                            + 8 * std::mem::size_of::<usize>()) as u64,
                    )
            })
    }
    pub fn query(&self) -> QueryId {
        self.query
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    pub fn outcome(self: &Arc<Self>, product: ProductRef) -> Option<FrontStorageGraphOutput> {
        let completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        let (selected, data) = completed.as_ref()?;
        (*selected == product).then(|| FrontStorageGraphOutput {
            profiles: data.profiles.clone(),
            diagnostics: data.diagnostics.clone(),
            reads: data.reads.clone(),
            _owner: self.clone(),
        })
    }
    pub fn failure(&self, reference: Reference) -> Option<QueryError> {
        let own = self
            .failure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|(selected, error)| (*selected == reference).then(|| error.clone()));
        own.or_else(|| self.recipes.failure(reference))
            .or_else(|| self.lower.failure(reference))
            .or_else(|| self.growth.failure(reference))
            .or_else(|| {
                self.front
                    .outcome(reference)
                    .and_then(|o| o.outcome.result.err())
            })
    }
    pub fn executions(&self) -> u64 {
        self.executions.load(Relaxed)
    }
    fn fail(&self, error: QueryError) -> Reference {
        let Some(bytes) = self.records.encode(&self.anchors, &error) else {
            return self.errors.resource;
        };
        if bytes.len() as u64 > self.output_budget {
            return self.errors.resource;
        }
        let Some(reference) = self.store.put_content(FAILURE, &bytes) else {
            return self.errors.resource;
        };
        *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some((reference, error));
        reference
    }
}
struct Factory(Arc<FrontStorageGraphQueries>);
impl QueryFactory for Factory {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if id != self.0.query {
            return Err(self.0.errors.protocol);
        }
        let terminal = self
            .0
            .completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|(product, _)| Ok(*product));
        Ok(Box::new(Task {
            owner: self.0.clone(),
            initialized: false,
            pending: BTreeSet::new(),
            seen: BTreeSet::new(),
            bodies: BTreeMap::new(),
            current: None,
            recipe: None,
            reads: ReadsBuilder::new(),
            waiting: None,
            input: None,
            terminal,
        }))
    }
}
struct Current {
    storage: StorageDefinition,
    body: usize,
    node: usize,
}
struct Task {
    owner: Arc<FrontStorageGraphQueries>,
    initialized: bool,
    pending: BTreeSet<DefPath>,
    seen: BTreeSet<DefPath>,
    bodies: BTreeMap<hirsel_types::storage::StorageCallable, StorageBody>,
    current: Option<Current>,
    recipe: Option<Box<dyn QueryTask>>,
    reads: ReadsBuilder,
    waiting: Option<QueryId>,
    input: Option<ProductRef>,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.owner.query
    }
    fn weight(&self) -> MemoryWeight {
        self.owner.weight
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
            Err(reference) => QueryPoll::Failed(reference),
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
        let owner = self.owner.clone();
        if !self.initialized {
            let product = match self.read(cx, owner.growth.query()) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(product)) => product,
            };
            self.input = Some(product);
            let mut checked = Vec::new();
            for path in &owner.definitions {
                let Some(output) = owner.front.checked_definition(path) else {
                    return Poll::Ready(Err(owner.fail(invalid(format!(
                        "missing checked source storage root {path}"
                    )))));
                };
                self.reads.merge(&output.reads);
                if output
                    .summary
                    .summary
                    .calls
                    .iter()
                    .any(|call| call.callee.path().is_core("region.with"))
                {
                    self.pending.insert(path.clone());
                }
                checked.push((path.clone(), output.summary));
            }
            // Actual callers, including unexecuted source roots, preserve the
            // original provenance of callback and owner arguments.
            loop {
                let mut changed = false;
                for (path, summary) in &checked {
                    if summary
                        .summary
                        .calls
                        .iter()
                        .any(|call| self.pending.contains(call.callee.path()))
                    {
                        changed |= self.pending.insert(path.clone());
                    }
                }
                if !changed {
                    break;
                }
            }
            self.initialized = true;
        }
        loop {
            if self.current.is_none() {
                let Some(path) = self.pending.pop_first() else {
                    return Poll::Ready(self.complete().map_err(|error| owner.fail(error)));
                };
                if !self.seen.insert(path.clone()) {
                    continue;
                }
                // Reviewed core recipes are independent. An instance body is
                // selected through the immutable instance table at each use.
                if path.0.package.name == "core" || owner.instances.contains(&path) {
                    continue;
                }
                let Some(query) = owner.recipes.query(&path) else {
                    return Poll::Ready(Err(owner.fail(invalid(format!(
                        "missing actual storage producer for {path}"
                    )))));
                };
                if self.recipe.is_none() {
                    self.recipe = match owner.recipes.factory().create(query) {
                        Ok(recipe) => Some(recipe),
                        Err(error) => return Poll::Ready(Err(error)),
                    };
                }
                let recipe = self.recipe.as_mut().unwrap();
                if recipe.id() != query {
                    return Poll::Ready(Err(owner.errors.protocol));
                }
                // The root owns one recipe continuation. Its reads are still
                // actual runtime reads; Pending releases this root's execution
                // grant instead of allocating a scheduler node per definition.
                let product = match recipe.poll(cx) {
                    QueryPoll::Waiting(dependencies) => {
                        let [dependency] = dependencies.as_slice() else {
                            return Poll::Ready(Err(owner.errors.protocol));
                        };
                        self.waiting = Some(*dependency);
                        self.pending.insert(path.clone());
                        self.seen.remove(&path);
                        return Poll::Pending;
                    }
                    QueryPoll::Failed(error) => return Poll::Ready(Err(error)),
                    QueryPoll::Complete(product) => product,
                };
                self.recipe = None;
                self.waiting = None;
                let Some(output) = owner.recipes.outcome(product) else {
                    return Poll::Ready(Err(owner.errors.protocol));
                };
                self.reads.merge(&output.reads);
                self.current = Some(Current {
                    storage: output.output.value.clone(),
                    body: 0,
                    node: 0,
                });
            }
            let mut current = self.current.take().unwrap();
            while current.body < current.storage.bodies.len() {
                let body = &mut current.storage.bodies[current.body];
                while current.node < body.nodes.len() {
                    let node = &mut body.nodes[current.node];
                    if let StorageOp::Assign(assignments) = &mut node.operation {
                        for (_, value) in assignments {
                            if let StorageValue::ConstantReference {
                                definition,
                                arguments,
                            } = value
                            {
                                if !arguments.is_empty() {
                                    *value = StorageValue::Unknown;
                                    continue;
                                }
                                let Some(query) = owner.lower.evaluate_query(definition.path())
                                else {
                                    return Poll::Ready(Err(owner.fail(invalid(format!(
                                        "missing source constant producer for {}",
                                        definition.path()
                                    )))));
                                };
                                match self.read(cx, query) {
                                    Poll::Pending => {
                                        self.current = Some(current);
                                        return Poll::Pending;
                                    }
                                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                                    Poll::Ready(Ok(_)) => (),
                                }
                                let materialized =
                                    match owner.lower.settled_constant(definition.path(), &[]) {
                                        Ok(value) => value,
                                        Err(error) => return Poll::Ready(Err(owner.fail(error))),
                                    };
                                if let Some(reads) = owner
                                    .lower
                                    .settled_reads(definition.path(), QueryKind::Evaluate)
                                {
                                    self.reads.merge(&reads);
                                }
                                let snapshot = owner.lower.signatures();
                                let mut provider = crate::FrozenProvider::new(
                                    &snapshot,
                                    definition.path().0.package.clone(),
                                    &mut self.reads,
                                );
                                *value = match crate::storage_graph::constant_value(
                                    snapshot.types(),
                                    &materialized,
                                    &mut |id| {
                                        hirsel_types::SignatureProvider::signature(
                                            &mut provider,
                                            id,
                                        )
                                    },
                                ) {
                                    Ok(value) => value,
                                    Err(error) => return Poll::Ready(Err(owner.fail(error))),
                                };
                            }
                        }
                    }
                    match &node.operation {
                        StorageOp::Call {
                            callee:
                                StorageCallee::Apply {
                                    definition,
                                    arguments,
                                },
                            ..
                        } => {
                            if let Err(error) = self.reference(*definition, arguments) {
                                return Poll::Ready(Err(owner.fail(error)));
                            }
                        }
                        StorageOp::Call {
                            callee: StorageCallee::Definition(definition),
                            ..
                        } => {
                            self.pending.insert(definition.path().clone());
                        }
                        StorageOp::Assign(assignments) => {
                            for (_, value) in assignments {
                                match value {
                                    StorageValue::Reference {
                                        definition,
                                        arguments,
                                    } => {
                                        if let Err(error) = self.reference(*definition, arguments) {
                                            return Poll::Ready(Err(owner.fail(error)));
                                        }
                                    }
                                    StorageValue::Constant { code, .. } => {
                                        for reference in code {
                                            if let Err(error) = self.reference(
                                                reference.definition,
                                                &reference.arguments,
                                            ) {
                                                return Poll::Ready(Err(owner.fail(error)));
                                            }
                                        }
                                    }
                                    StorageValue::Callable { target, .. } => {
                                        self.pending.insert(target.definition.path().clone());
                                    }
                                    _ => (),
                                }
                            }
                        }
                        _ => (),
                    }
                    current.node += 1;
                }
                if self.bodies.insert(body.callable, body.clone()).is_some() {
                    return Poll::Ready(Err(
                        owner.fail(invalid("duplicate actual callable association"))
                    ));
                }
                current.body += 1;
                current.node = 0;
            }
            let bytes = owner.records.encode(&owner.anchors, &self.bodies);
            if bytes.is_none_or(|bytes| bytes.len() as u64 > owner.output_budget) {
                return Poll::Ready(Err(owner.errors.resource));
            }
        }
    }
    fn reference(
        &mut self,
        definition: DefId,
        arguments: &[hirsel_types::GenericArgument],
    ) -> Result<(), QueryError> {
        self.pending.insert(definition.path().clone());
        if definition.path().0.package.name == "core"
            || !self.owner.instances.contains(definition.path())
        {
            return Ok(());
        }
        let signature = self.owner.lower.signature(definition.path())?;
        let hirsel_types::SignatureKind::Function(function) = &signature.kind else {
            return Ok(());
        };
        if function.generics.len() != arguments.len()
            || !function.generics.iter().zip(arguments).all(|(p, a)| {
                matches!(
                    (p, a),
                    (
                        hirsel_types::GenericParameter::Type { .. },
                        hirsel_types::GenericArgument::Type(_)
                    ) | (
                        hirsel_types::GenericParameter::Size { .. },
                        hirsel_types::GenericArgument::Size(_)
                    )
                )
            })
        {
            return Ok(());
        }
        if hirsel_tir::InstanceBodies::key(self.owner.lower.signatures().types(), arguments)
            .is_none()
        {
            return Ok(());
        }
        let selected = self
            .owner
            .lower
            .settled_instance(definition.path(), arguments)?
            .ok_or_else(|| {
                invalid(format!(
                    "missing selected generated storage instance for {}",
                    definition.path()
                ))
            })?;
        self.pending.insert(selected);
        Ok(())
    }
    fn complete(&mut self) -> Result<ProductRef, QueryError> {
        let owner = &self.owner;
        let bodies = std::mem::take(&mut self.bodies)
            .into_values()
            .collect::<Vec<_>>();
        let lower = owner.lower.clone();
        let select = |definition: DefId, arguments: &[hirsel_types::GenericArgument]| {
            lower
                .settled_instance(definition.path(), arguments)
                .ok()
                .map(|selected| selected.map_or(definition, |p| DefId::intern(&p)))
        };
        owner.executions.fetch_add(1, Relaxed);
        let (profiles, diagnostics) =
            crate::storage_graph::finish(owner.lower.signatures().types(), &select, bodies)?;
        let reads = std::mem::take(&mut self.reads)
            .finish()
            .map_err(|_| invalid("storage source reads disagree"))?;
        let bytes = owner
            .records
            .encode(&owner.anchors, &(&profiles, &diagnostics, &reads))
            .ok_or(QueryError::NotImplemented)?;
        if bytes.len() as u64 > owner.output_budget {
            return Err(invalid("storage graph output exceeds admission"));
        }
        let record = owner
            .store
            .put_content(OUTPUT, &bytes)
            .ok_or(QueryError::NotImplemented)?;
        let input = self.input.ok_or(QueryError::NotImplemented)?;
        let action = hirsel_artifact_store::Action(ContentHash::of_parts(
            "hirsel.front.storage-graph.action.v1",
            [input.action.0.0.as_slice(), record.output.0.0.as_slice()],
        ));
        let product = ProductRef {
            address: owner.query.address,
            action,
            record,
        };
        *owner.completed.lock().unwrap_or_else(|e| e.into_inner()) = Some((
            product,
            Arc::new(Data {
                profiles: Arc::new(profiles),
                diagnostics: Arc::new(diagnostics),
                reads: Arc::new(reads),
            }),
        ));
        owner
            .store
            .associate_product(product.address, product.action, product.record);
        Ok(product)
    }
}
fn invalid(message: impl Into<String>) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "admitted source storage proof".into(),
        message: message.into(),
    })
}
impl QueryEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn front_storage_graph_queries(
        &mut self,
        root: &DefPath,
        front: Arc<FrontBodyQueries>,
        growth: Arc<FrontGrowthQueries>,
        recipes: Arc<FrontStorageQueries>,
        lower: Arc<FrontLowerQueries>,
        weight: MemoryWeight,
        allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<FrontStorageGraphQueries>, QueryError> {
        if !recipes.fits_poll(weight) {
            return Err(invalid(
                "storage recipe exceeds its parent execution admission",
            ));
        }
        if !Arc::ptr_eq(&lower.signatures(), &self.signature_inputs()) {
            return Err(invalid(
                "storage and Lower require the same signature snapshot",
            ));
        }
        let definitions = self.front_source_roots();
        let inventory = FrontStorageGraphQueries::input_bytes(&definitions)
            .ok_or(QueryError::NotImplemented)?;
        let output_budget = allowance
            .bytes()
            .checked_sub(inventory)
            .ok_or_else(|| invalid("source storage inventory exceeds admission"))?;
        let instances = definitions
            .iter()
            .filter(|p| self.source_facts(p).is_some_and(|f| f.instance_bodied))
            .cloned()
            .collect();
        let store = self
            .result_store
            .clone()
            .filter(|s| s.enabled())
            .ok_or(QueryError::NotImplemented)?;
        let query = QueryId {
            kind: FRONT_STORAGE_GRAPH_KIND,
            address: hirsel_artifact_store::Address(ContentHash::of_parts(
                "hirsel.front.storage-graph.address.v1",
                [
                    self.environment_value().0.as_slice(),
                    root.fingerprint().0.as_slice(),
                ],
            )),
        };
        Ok(Arc::new(FrontStorageGraphQueries {
            query,
            definitions,
            instances,
            front,
            growth,
            recipes,
            lower,
            records: crate::NativeRecords::new(self.types.clone(), store.clone()),
            anchors: crate::NativeRecordContext::capture(self),
            store,
            completed: Mutex::new(None),
            failure: Mutex::new(None),
            executions: AtomicU64::new(0),
            weight,
            errors,
            output_budget,
            _allowance: allowance,
        }))
    }
}
