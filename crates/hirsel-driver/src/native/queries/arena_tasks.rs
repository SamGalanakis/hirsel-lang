//! Actual component solver continuations after native instance discovery.
//! Topology planning opens no equations. Candidates/Chosen execute only after
//! their explicit instance, same-stratum outgoing and own-Candidates reads.
use super::{
    arena::{ArenaQueryCounts, EquationProduct},
    graph::{ArenaNode, ArenaQueryPlan, SettledArenaGraph},
    instance_tasks::InstanceTasks,
    *,
};
use hirsel_engine::store::{Action, Address, Domain};
use hirsel_engine::{runtime::*, store::ResultStore};
use std::{
    collections::BTreeMap,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    task::Poll,
};

pub const ARENA_GRAPH_KIND: u32 = 0x4e00_0022;
struct Plan {
    discovery: ProductRef,
    queries: ArenaQueryPlan,
    ids: Vec<QueryId>,
    index: BTreeMap<(u32, ContentHash), usize>,
    instances: BTreeMap<ContentHash, ProductRef>,
    outputs: Vec<OnceLock<Arc<ArenaSummary>>>,
}
pub struct ArenaTasks {
    instances: Arc<InstanceTasks>,
    store: Arc<dyn ResultStore>,
    facet: u32,
    query: QueryId,
    plan: OnceLock<Plan>,
    completed: OnceLock<(ProductRef, SettledArenaGraph)>,
    weight: MemoryWeight,
    allowance: Arc<MemoryReservation>,
    charged: AtomicU64,
    fallback: Reference,
    pub hits: AtomicU64,
    pub solves: AtomicU64,
}
fn component_id(key: &ComponentKey) -> Result<QueryId, String> {
    Ok(QueryId {
        kind: match key.stratum {
            ArenaStratum::Candidates => ARENA_CANDIDATES_KIND,
            ArenaStratum::Chosen => ARENA_CHOSEN_KIND,
        },
        address: key.address()?,
    })
}
impl ArenaTasks {
    pub fn new(
        instances: Arc<InstanceTasks>,
        store: Arc<dyn ResultStore>,
        facet: u32,
        weight: MemoryWeight,
        allowance: MemoryReservation,
    ) -> Result<Arc<Self>, String> {
        if weight.retained < std::mem::size_of::<ArenaTask>() as u64 {
            return Err("arena continuation exceeds retained allowance".into());
        }
        let source = instances.discovery_id().address.0;
        let query = QueryId {
            kind: ARENA_GRAPH_KIND,
            address: Address(ContentHash::of_parts(
                "hirsel.native.arena-graph-address.v1",
                [source.0.as_slice(), facet.to_le_bytes().as_slice()],
            )),
        };
        let fallback = instances.failure("arena task resource or protocol refusal");
        Ok(Arc::new(Self {
            instances,
            store,
            facet,
            query,
            plan: OnceLock::new(),
            completed: OnceLock::new(),
            weight,
            allowance: Arc::new(allowance),
            charged: AtomicU64::new(0),
            fallback,
            hits: AtomicU64::new(0),
            solves: AtomicU64::new(0),
        }))
    }
    pub(super) fn retained_allowance(&self) -> Arc<MemoryReservation> {
        self.allowance.clone()
    }
    pub fn query_id(&self) -> QueryId {
        self.query
    }
    pub fn graph(&self, selected: ProductRef) -> Result<SettledArenaGraph, String> {
        let (product, graph) = self
            .completed
            .get()
            .ok_or("arena graph has not completed")?;
        if *product != selected {
            return Err("arena graph differs from settled output".into());
        }
        Ok(graph.clone())
    }
    fn charge(&self, bytes: u64) -> Result<(), String> {
        self.charged
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |charged| {
                charged
                    .checked_add(bytes)
                    .filter(|total| *total <= self.allowance.bytes())
            })
            .map(|_| ())
            .map_err(|charged| format!(
                "arena descriptor metadata allowance exhausted: requested {bytes} bytes, retained {charged}, allowance {}", self.allowance.bytes()
            ))
    }
    fn plan(&self, discovery: ProductRef) -> Result<&Plan, String> {
        if let Some(plan) = self.plan.get() {
            return if plan.discovery == discovery {
                Ok(plan)
            } else {
                Err("arena discovery changed within immutable snapshot".into())
            };
        }
        let completed = self.instances.completed_instances(discovery.clone())?;
        let nodes = completed
            .iter()
            .map(|(key, output)| ArenaNode {
                equations: EquationProduct {
                    owner: key.clone(),
                    reference: output.product.local_equations,
                },
                callees: output.facts.body.callees.clone(),
            })
            .collect::<Vec<_>>();
        let mut queries = ArenaQueryPlan::new(&nodes, &[], self.facet)?;
        let ids = queries
            .component_keys
            .iter()
            .map(component_id)
            .collect::<Result<Vec<_>, _>>()?;
        if (std::mem::size_of::<ArenaTask>() + ids.len() * 256) as u64 > self.weight.scratch {
            return Err("arena graph reads exceed scratch allowance".into());
        }
        for (index, key) in queries.component_keys.iter().enumerate() {
            let reads = key.members.len() + queries.dependencies(index)?.len();
            if (std::mem::size_of::<ArenaTask>() + reads * 256) as u64 > self.weight.scratch {
                return Err("arena component reads exceed scratch allowance".into());
            }
        }
        queries.retain_owner(self.allowance.clone());
        let index = ids
            .iter()
            .enumerate()
            .map(|(index, id)| ((id.kind, id.address.0), index))
            .collect();
        let outputs = (0..ids.len()).map(|_| OnceLock::new()).collect();
        let instances = completed
            .into_iter()
            .map(|(key, output)| (key.address(INSTANCE_KIND).0, output.product.product.clone()))
            .collect();
        let mut plan = Plan {
            discovery,
            queries,
            ids,
            index,
            instances,
            outputs,
        };
        let mut weight = hirsel_engine::NativeMetadataWeight::default();
        weight.add(size_of::<Plan>());
        weight.add(plan.queries.retained_storage_bytes() as usize);
        weight.flat_vec(&plan.ids);
        weight.packed_map(&mut plan.index, |_, _, _| ());
        weight.packed_map(&mut plan.instances, |_, _, _| ());
        weight.flat_vec(&plan.outputs);
        self.charge(weight.bytes())?;
        self.plan
            .set(plan)
            .map_err(|_| "duplicate arena plan construction")?;
        Ok(self.plan.get().unwrap())
    }
    fn publish_graph(
        &self,
        plan: &Plan,
        selected: &BTreeMap<usize, Arc<ArenaSummary>>,
    ) -> Result<ProductRef, String> {
        let mut graph = plan.queries.finish(selected)?;
        let bytes = graph.retained_index_bytes();
        self.charge(bytes)?;
        let rows = plan
            .ids
            .iter()
            .enumerate()
            .map(|(index, id)| {
                selected
                    .get(&index)
                    .map(|summary| (id.kind, id.address.0, summary.product.record))
                    .ok_or("arena component has not completed")
            })
            .collect::<Result<Vec<_>, _>>()?;
        let bytes = serde_json::to_vec(&rows).map_err(|e| e.to_string())?;
        let domain = Domain {
            kind: ARENA_GRAPH_KIND,
            schema: 1,
        };
        let record = self
            .store
            .put_content(domain, &bytes)
            .ok_or("arena graph publication failed")?;
        if domain.reference(&bytes) != Some(record) {
            return Err("arena graph content identity differs".into());
        }
        let action = Action(ContentHash::of_parts(
            "hirsel.native.arena-graph-action.v1",
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
        if let Err(other) = self.completed.set((product.clone(), graph))
            && self.completed.get().unwrap().0 != other.0
        {
            return Err("arena graph completion changed".into());
        }
        Ok(product)
    }
}
pub struct ArenaTaskFactory {
    tasks: Arc<ArenaTasks>,
}
impl ArenaTaskFactory {
    pub fn new(tasks: Arc<ArenaTasks>) -> Self {
        Self { tasks }
    }
}
impl QueryFactory for ArenaTaskFactory {
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
        Ok(Box::new(ArenaTask {
            tasks: self.tasks.clone(),
            key,
            index,
            terminal: None,
        }))
    }
}
struct ArenaTask {
    tasks: Arc<ArenaTasks>,
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
impl QueryTask for ArenaTask {
    fn id(&self) -> QueryId {
        self.key
    }
    fn weight(&self) -> MemoryWeight {
        // Registry descriptors have their own reservation. This task retains
        // only Arc handles, its identity and a terminal result across polls.
        MemoryWeight {
            retained: std::mem::size_of::<Self>() as u64,
            ..self.tasks.weight
        }
    }
    fn continuation_bytes(&self) -> u64 {
        std::mem::size_of::<Self>() as u64
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if let Some(result) = self.terminal.clone() {
            return finish(result);
        }
        let result = self.advance(cx);
        match result {
            Ok(Ok(product)) => {
                self.terminal = Some(Ok(product.clone()));
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
impl ArenaTask {
    fn advance(
        &self,
        cx: &mut dyn QueryContext,
    ) -> Result<Result<ProductRef, Vec<QueryId>>, Reference> {
        let tasks = &self.tasks;
        let fail = |e: String| tasks.instances.failure(&e);
        let plan = if self.index.is_none() {
            let discovery = tasks.instances.discovery_id();
            let product = match cx.read(discovery) {
                Poll::Pending => return Ok(Err(vec![discovery])),
                Poll::Ready(Err(error)) => return Err(error),
                Poll::Ready(Ok(product)) => product,
            };
            tasks.plan(product).map_err(fail)?
        } else {
            tasks
                .plan
                .get()
                .ok_or_else(|| fail("arena plan is unavailable".into()))?
        };
        let reads = if let Some(index) = self.index {
            plan.queries.dependencies(index).map_err(fail)?
        } else {
            (0..plan.ids.len()).collect()
        };
        let mut selected = BTreeMap::new();
        let mut waiting = Vec::new();
        if let Some(index) = self.index {
            for member in &plan.queries.component_keys[index].members {
                let expected = plan.instances[&member.address(INSTANCE_KIND).0].clone();
                let query = instance_query(member, expected.clone());
                match cx.read(query) {
                    Poll::Pending => waiting.push(query),
                    Poll::Ready(Err(error)) => return Err(error),
                    Poll::Ready(Ok(product)) if expected == product => {}
                    _ => return Err(fail("arena member differs from discovered instance".into())),
                }
            }
        }
        for index in reads {
            let query = plan.ids[index];
            match cx.read(query) {
                Poll::Pending => waiting.push(query),
                Poll::Ready(Err(error)) => return Err(error),
                Poll::Ready(Ok(product)) => {
                    let summary = plan.outputs[index].get().ok_or_else(|| {
                        fail("arena dependency has no completed descriptor".into())
                    })?;
                    if summary.product != product {
                        return Err(fail("arena dependency output differs".into()));
                    }
                    selected.insert(index, summary.clone());
                }
            }
        }
        if !waiting.is_empty() {
            return Ok(Err(waiting));
        }
        let result = if let Some(index) = self.index {
            let mut counts = ArenaQueryCounts::default();
            let mut summary = plan
                .queries
                .solve(tasks.store.as_ref(), index, &selected, &mut counts)
                .map_err(fail)?;
            tasks.hits.fetch_add(counts.hits, Ordering::Relaxed);
            tasks.solves.fetch_add(counts.misses, Ordering::Relaxed);
            let output = Arc::make_mut(&mut summary);
            output.retained_owner = Some(tasks.allowance.clone());
            tasks
                .charge(output.retained_metadata_bytes())
                .map_err(fail)?;
            let product = summary.product.clone();
            if let Err(other) = plan.outputs[index].set(summary)
                && plan.outputs[index].get().unwrap().product != other.product
            {
                return Err(fail("arena component completion changed".into()));
            }
            product
        } else {
            tasks.publish_graph(plan, &selected).map_err(fail)?
        };
        Ok(Ok(result))
    }
}
