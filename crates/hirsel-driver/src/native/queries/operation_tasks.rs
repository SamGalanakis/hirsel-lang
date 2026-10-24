//! Operation-freedom continuations over actual cold instance producers.
//! Factories create frames only; graph and component reads suspend explicitly.
use super::{
    instance_tasks::InstanceTasks,
    operation::{OperationCounts, OperationQueryPlan, OperationResult, SettledOperations},
    *,
};
use hirsel_artifact_store::{Action, Address, Domain};
use hirsel_engine::{runtime::*, store::ResultStore};
use std::{
    collections::BTreeMap,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    task::Poll,
};

pub const OPERATION_GRAPH_KIND: u32 = 0x4e00_0027;
struct Plan {
    discovery: ProductRef,
    queries: OperationQueryPlan,
    ids: Vec<QueryId>,
    index: BTreeMap<(u32, ContentHash), usize>,
    outputs: Vec<OnceLock<Arc<OperationResult>>>,
}
pub struct OperationTasks {
    instances: Arc<InstanceTasks>,
    store: Arc<dyn ResultStore>,
    query: QueryId,
    plan: OnceLock<Plan>,
    completed: OnceLock<(ProductRef, Arc<SettledOperations>)>,
    weight: MemoryWeight,
    allowance: Arc<MemoryReservation>,
    fallback: Reference,
    pub hits: AtomicU64,
    pub publications: AtomicU64,
    pub evaluations: AtomicU64,
}
impl OperationTasks {
    pub fn new(
        instances: Arc<InstanceTasks>,
        store: Arc<dyn ResultStore>,
        weight: MemoryWeight,
        allowance: MemoryReservation,
    ) -> Result<Arc<Self>, String> {
        if weight.retained < std::mem::size_of::<OperationTask>() as u64 {
            return Err("operation continuation exceeds retained allowance".into());
        }
        let source = instances.discovery_id().address.0;
        let query = QueryId {
            kind: OPERATION_GRAPH_KIND,
            address: Address(ContentHash::of_parts(
                "hirsel.native.operation-graph-address.v1",
                [source.0.as_slice()],
            )),
        };
        let fallback = instances.failure("operation task resource or protocol refusal");
        Ok(Arc::new(Self {
            instances,
            store,
            query,
            plan: OnceLock::new(),
            completed: OnceLock::new(),
            weight,
            allowance: Arc::new(allowance),
            fallback,
            hits: AtomicU64::new(0),
            publications: AtomicU64::new(0),
            evaluations: AtomicU64::new(0),
        }))
    }
    pub fn query_id(&self) -> QueryId {
        self.query
    }
    pub fn graph(&self, selected: ProductRef) -> Result<Arc<SettledOperations>, String> {
        let (product, graph) = self
            .completed
            .get()
            .ok_or("operation graph has not completed")?;
        if *product != selected {
            return Err("operation graph differs from selected output".into());
        }
        Ok(graph.clone())
    }
    fn plan(&self, discovery: ProductRef) -> Result<&Plan, String> {
        if let Some(plan) = self.plan.get() {
            return if plan.discovery == discovery {
                Ok(plan)
            } else {
                Err("operation discovery changed within immutable snapshot".into())
            };
        }
        let declarations = &self.instances.source_inputs().declarations;
        let operations = declarations.program.operation_registry();
        let manifest = declarations.manifest()?;
        let environment = hirsel_lir::validate::NativeEnvironment {
            target: &self.instances.options().target,
            manifest: &manifest,
            operations: &operations,
            discharge: Some(&self.instances.options().discharge),
        };
        let inputs = self.instances.products(discovery)?;
        let queries = OperationQueryPlan::new(&inputs, &environment)?;
        let ids = queries.ids();
        if (std::mem::size_of::<OperationTask>() + ids.len() * 256) as u64 > self.weight.retained {
            return Err("operation graph reads exceed retained allowance".into());
        }
        for index in 0..ids.len() {
            let reads = queries.instances(index)?.len() + queries.dependencies(index)?.len();
            if (std::mem::size_of::<OperationTask>() + reads * 256) as u64 > self.weight.retained {
                return Err("operation component reads exceed retained allowance".into());
            }
        }
        if queries.retained_bytes()? > self.allowance.bytes() {
            return Err("operation descriptor metadata allowance exhausted".into());
        }
        let index = ids
            .iter()
            .enumerate()
            .map(|(i, id)| ((id.kind, id.address.0), i))
            .collect();
        let outputs = (0..ids.len()).map(|_| OnceLock::new()).collect();
        self.plan
            .set(Plan {
                discovery,
                queries,
                ids,
                index,
                outputs,
            })
            .map_err(|_| "duplicate operation plan construction")?;
        Ok(self.plan.get().unwrap())
    }
    fn publish_graph(
        &self,
        plan: &Plan,
        selected: &BTreeMap<usize, Arc<OperationResult>>,
    ) -> Result<ProductRef, String> {
        let mut graph = plan.queries.finish(selected)?;
        graph.retained_allowance = Some(self.allowance.clone());
        let rows = plan
            .ids
            .iter()
            .enumerate()
            .map(|(index, id)| {
                selected
                    .get(&index)
                    .map(|output| (id.address.0, output.summary.product.record))
                    .ok_or("operation component has not completed")
            })
            .collect::<Result<Vec<_>, _>>()?;
        let bytes = serde_json::to_vec(&rows).map_err(|e| e.to_string())?;
        let domain = Domain {
            kind: OPERATION_GRAPH_KIND,
            schema: 1,
        };
        let record = self
            .store
            .put_content(domain, &bytes)
            .ok_or("operation graph publication failed")?;
        if domain.reference(&bytes) != Some(record) {
            return Err("operation graph content identity differs".into());
        }
        let action = Action(ContentHash::of_parts(
            "hirsel.native.operation-graph-action.v1",
            [
                self.query.address.0.0.as_slice(),
                record.output.0.0.as_slice(),
            ],
        ));
        self.store
            .associate_product(self.query.address, action, record);
        let product = ProductRef {
            address: self.query.address,
            action,
            record,
        };
        if let Err(other) = self.completed.set((product, Arc::new(graph)))
            && self.completed.get().unwrap().0 != other.0
        {
            return Err("operation graph completion changed".into());
        }
        Ok(product)
    }
}
pub struct OperationTaskFactory {
    tasks: Arc<OperationTasks>,
}
impl OperationTaskFactory {
    pub fn new(tasks: Arc<OperationTasks>) -> Self {
        Self { tasks }
    }
}
impl QueryFactory for OperationTaskFactory {
    fn create(&self, key: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        let index = if key == self.tasks.query {
            None
        } else {
            Some(
                *self
                    .tasks
                    .plan
                    .get()
                    .and_then(|plan| plan.index.get(&(key.kind, key.address.0)))
                    .ok_or(self.tasks.fallback)?,
            )
        };
        Ok(Box::new(OperationTask {
            tasks: self.tasks.clone(),
            key,
            index,
            terminal: None,
        }))
    }
}
struct OperationTask {
    tasks: Arc<OperationTasks>,
    key: QueryId,
    index: Option<usize>,
    terminal: Option<Result<ProductRef, Reference>>,
}
fn finish(result: Result<ProductRef, Reference>) -> QueryPoll {
    match result {
        Ok(product) => QueryPoll::Complete(product),
        Err(error) => QueryPoll::Failed(error),
    }
}
impl QueryTask for OperationTask {
    fn id(&self) -> QueryId {
        self.key
    }
    fn weight(&self) -> MemoryWeight {
        self.tasks.weight
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if let Some(result) = self.terminal {
            return finish(result);
        }
        match self.advance(cx) {
            Ok(Ok(product)) => {
                self.terminal = Some(Ok(product));
                finish(Ok(product))
            }
            Ok(Err(waiting)) => QueryPoll::Waiting(waiting),
            Err(error) => {
                self.terminal = Some(Err(error));
                finish(Err(error))
            }
        }
    }
}
impl OperationTask {
    fn advance(
        &self,
        cx: &mut dyn QueryContext,
    ) -> Result<Result<ProductRef, Vec<QueryId>>, Reference> {
        let tasks = &self.tasks;
        let fail = |e: String| tasks.instances.failure(&e);
        let plan = if self.index.is_none() {
            let discovery = tasks.instances.discovery_id();
            let selected = match cx.read(discovery) {
                Poll::Pending => return Ok(Err(vec![discovery])),
                Poll::Ready(Err(error)) => return Err(error),
                Poll::Ready(Ok(product)) => product,
            };
            tasks.plan(selected).map_err(fail)?
        } else {
            tasks
                .plan
                .get()
                .ok_or_else(|| fail("operation plan is unavailable".into()))?
        };
        let reads = if let Some(index) = self.index {
            plan.queries.dependencies(index).map_err(fail)?.to_vec()
        } else {
            (0..plan.ids.len()).collect()
        };
        let mut waiting = Vec::new();
        let mut selected = BTreeMap::new();
        if let Some(index) = self.index {
            for expected in plan.queries.instances(index).map_err(fail)? {
                let query = QueryId {
                    kind: INSTANCE_KIND,
                    address: expected.address,
                };
                match cx.read(query) {
                    Poll::Pending => waiting.push(query),
                    Poll::Ready(Err(error)) => return Err(error),
                    Poll::Ready(Ok(product)) if product == *expected => {}
                    _ => {
                        return Err(fail(
                            "operation member differs from discovered instance".into(),
                        ));
                    }
                }
            }
        }
        for index in reads {
            let query = plan.ids[index];
            match cx.read(query) {
                Poll::Pending => waiting.push(query),
                Poll::Ready(Err(error)) => return Err(error),
                Poll::Ready(Ok(product)) => {
                    let output = plan.outputs[index].get().ok_or_else(|| {
                        fail("operation dependency has no completed descriptor".into())
                    })?;
                    if output.summary.product != product {
                        return Err(fail("operation dependency output differs".into()));
                    }
                    selected.insert(index, output.clone());
                }
            }
        }
        if !waiting.is_empty() {
            return Ok(Err(waiting));
        }
        let product = if let Some(index) = self.index {
            let mut counts = OperationCounts::default();
            let output = plan
                .queries
                .solve(tasks.store.as_ref(), index, &selected, &mut counts)
                .map_err(fail)?;
            tasks.hits.fetch_add(counts.hits, Ordering::Relaxed);
            tasks
                .publications
                .fetch_add(counts.publications, Ordering::Relaxed);
            tasks
                .evaluations
                .fetch_add(counts.evaluations, Ordering::Relaxed);
            let product = output.summary.product;
            if let Err(other) = plan.outputs[index].set(output)
                && plan.outputs[index].get().unwrap().summary.product != other.summary.product
            {
                return Err(fail("operation component completion changed".into()));
            }
            product
        } else {
            tasks.publish_graph(plan, &selected).map_err(fail)?
        };
        Ok(Ok(product))
    }
}
