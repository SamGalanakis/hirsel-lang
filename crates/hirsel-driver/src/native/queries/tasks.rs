//! Native Ready continuations over an immutable admitted input snapshot.
//! Input tasks re-admit the already produced base/SCC descriptors; source
//! discovery and their cold producers still precede this snapshot boundary.
use super::{
    products::{DemandProducts, NativeSnapshot, SettledProducts},
    *,
};
use crate::native::object_queries::{
    CodegenKey, LinkDependencies, LinkInputs, LinkKey, ObjectProducts, ObjectQueries,
    ObjectTaskRequest, ObjectTaskSnapshot,
};
use hirsel_engine::{runtime::*, store::ResultStore};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
    task::Poll,
};

const FAILURE: hirsel_engine::store::Domain = hirsel_engine::store::Domain {
    kind: 0x4e00_001e,
    schema: 1,
};
type Id = (u32, ContentHash);
fn id(key: QueryId) -> Id {
    (key.kind, key.address.0)
}
fn query(kind: u32, product: ProductRef) -> QueryId {
    QueryId {
        kind,
        address: product.address,
    }
}
fn owner(key: &InstanceAddress) -> hirsel_rc::arena_credit::equations::Function {
    (key.instance.definition().clone(), key.instance.digest())
}
enum Work {
    Instance(InstanceAddress),
    Arena(ComponentKey),
    Ready(InstanceAddress),
}
struct Request {
    key: QueryId,
    work: Work,
    reads: Vec<(QueryId, hirsel_engine::store::Reference)>,
    guard: Option<QueryId>,
}

/// Immutable dispatch/read plans, built during snapshot admission. Factory
/// creation below performs only a bounded lookup and boxes one continuation.
pub struct NativeTasks {
    pub snapshot: Arc<NativeSnapshot>,
    requests: BTreeMap<Id, Arc<Request>>,
    weight: MemoryWeight,
    fallback: hirsel_engine::store::Reference,
    pub created: AtomicU64,
    pub ready_transformations: AtomicU64,
}
impl NativeTasks {
    pub(super) fn retained_dispatch_bytes(&mut self) -> u64 {
        use super::ownership_compilation::metadata as m;
        let mut w = hirsel_engine::NativeMetadataWeight::default();
        w.add(size_of::<Self>() + 2 * size_of::<usize>());
        w.packed_map(&mut self.requests, |w, _, request| {
            w.arc(request);
            w.flat_vec(&request.reads);
            match &request.work {
                Work::Instance(key) | Work::Ready(key) => m::address(w, key),
                Work::Arena(key) => m::component(w, key),
            }
        });
        w.bytes()
    }
    pub fn new(snapshot: Arc<NativeSnapshot>, weight: MemoryWeight) -> Result<Arc<Self>, String> {
        let fallback = snapshot
            .store
            .put_content(FAILURE, b"native task resource or protocol failure")
            .ok_or("native task error publication failed")?;
        let inputs = snapshot.instances.completed_inputs()?;
        let guard = if snapshot.needs_guard {
            if let Some(guard) = snapshot.guard.get() {
                Some(guard.clone())
            } else {
                let mut locals = inputs
                    .iter()
                    .map(|(_, output)| super::seal_demand::Input {
                        product: output.product.product,
                        equations: output.seal_demand.clone(),
                    })
                    .collect::<Vec<_>>();
                locals.sort_by(|a, b| a.equations.owner().cmp(b.equations.owner()));
                let mut roots = snapshot
                    .instances
                    .original_roots()
                    .iter()
                    .map(owner)
                    .collect::<Vec<_>>();
                roots.sort();
                roots.dedup();
                if roots.is_empty() {
                    return Err("sealed guard graph has no original entry selection".into());
                }
                let plan = super::seal_demand::Plan::new(
                    locals,
                    &roots,
                    snapshot.options.target.digest(),
                    inputs[0].0.policy,
                )?;
                let guard = super::seal_demand::GuardTasks::new(
                    snapshot.store.clone(),
                    plan,
                    weight,
                    snapshot.guard_admitted_bytes,
                    snapshot.retained_allowances.clone(),
                )?;
                snapshot
                    .guard
                    .set(guard.clone())
                    .map_err(|_| "seal-demand dispatch installed twice")?;
                Some(guard)
            }
        } else {
            None
        };
        let by_owner = inputs
            .iter()
            .map(|(key, output)| (owner(key), (key, output)))
            .collect::<BTreeMap<_, _>>();
        let addresses = snapshot
            .arena
            .component_keys
            .iter()
            .map(ComponentKey::address)
            .collect::<Result<Vec<_>, _>>()?;
        let components = snapshot
            .arena
            .component_keys
            .iter()
            .enumerate()
            .flat_map(|(index, key)| {
                key.members.iter().map(move |member| {
                    ((key.stratum == ArenaStratum::Chosen, owner(member)), index)
                })
            })
            .collect::<BTreeMap<_, _>>();
        let mut requests = BTreeMap::new();
        let summary_read = |index: usize| -> Result<_, String> {
            let key = &snapshot.arena.component_keys[index];
            let address = addresses[index];
            let summary = snapshot
                .summaries
                .get(&address.0)
                .ok_or("missing native component result")?;
            Ok((
                query(
                    if key.stratum == ArenaStratum::Chosen {
                        ARENA_CHOSEN_KIND
                    } else {
                        ARENA_CANDIDATES_KIND
                    },
                    summary.product,
                ),
                summary.product.record,
            ))
        };
        for (key, output) in &inputs {
            let q = query(INSTANCE_KIND, output.product.product);
            requests.insert(
                id(q),
                Arc::new(Request {
                    key: q,
                    work: Work::Instance(key.clone()),
                    reads: vec![],
                    guard: None,
                }),
            );
            let mut reads = BTreeMap::new();
            for member in std::iter::once(owner(key)).chain(output.callees.iter().cloned()) {
                let (_, input) = by_owner
                    .get(&member)
                    .ok_or("missing Ready instance dependency")?;
                let q = query(INSTANCE_KIND, input.product.product);
                reads.insert(id(q), (q, input.product.product.record));
                let component = components
                    .get(&(true, member))
                    .ok_or("missing Ready chosen component")?;
                let read = summary_read(*component)?;
                reads.insert(id(read.0), read);
            }
            let q = key.ready_query_id();
            requests.insert(
                id(q),
                Arc::new(Request {
                    key: q,
                    work: Work::Ready(key.clone()),
                    reads: reads.into_values().collect(),
                    guard: guard.as_ref().map(|g| g.query_id()),
                }),
            );
        }
        for (index, key) in snapshot.arena.component_keys.iter().enumerate() {
            let mut reads = BTreeMap::new();
            for member in &key.members {
                let (_, input) = by_owner
                    .get(&owner(member))
                    .ok_or("missing component instance")?;
                let q = query(INSTANCE_KIND, input.product.product);
                reads.insert(id(q), (q, input.product.product.record));
                for callee in &input.callees {
                    let component = components
                        .get(&(key.stratum == ArenaStratum::Chosen, callee.clone()))
                        .ok_or("missing outgoing component")?;
                    if addresses[*component] != addresses[index] {
                        let read = summary_read(*component)?;
                        reads.insert(id(read.0), read);
                    }
                }
            }
            if key.stratum == ArenaStratum::Chosen {
                let candidate = components
                    .get(&(false, owner(&key.members[0])))
                    .ok_or("missing own Candidates component")?;
                let read = summary_read(*candidate)?;
                reads.insert(id(read.0), read);
            }
            let (q, _) = summary_read(index)?;
            requests.insert(
                id(q),
                Arc::new(Request {
                    key: q,
                    work: Work::Arena(key.clone()),
                    reads: reads.into_values().collect(),
                    guard: None,
                }),
            );
        }
        if requests.values().any(|request| {
            (std::mem::size_of::<NativeTask>()
                + request.reads.len() * std::mem::size_of::<(QueryId, Reference)>())
                as u64
                > weight.retained
        }) {
            return Err("native continuation exceeds its declared retained allowance".into());
        }
        Ok(Arc::new(Self {
            snapshot,
            requests,
            weight,
            fallback,
            created: AtomicU64::new(0),
            ready_transformations: AtomicU64::new(0),
        }))
    }
    pub(super) fn ownership_reads(
        &self,
        owner: &InstanceAddress,
    ) -> Result<Vec<(QueryId, Reference)>, String> {
        let request = self
            .requests
            .get(&id(owner.ready_query_id()))
            .ok_or("ownership diagnostic has no settled Ready dependency plan")?;
        Ok(request.reads.clone())
    }
    pub fn guard_factory(&self) -> Option<(QueryId, Arc<dyn QueryFactory>)> {
        self.snapshot
            .guard
            .get()
            .map(|guard| (guard.query_id(), guard.factory()))
    }
    pub fn failure(&self, message: &str) -> hirsel_engine::store::Reference {
        self.snapshot
            .store
            .put_content(FAILURE, message.as_bytes())
            .unwrap_or(self.fallback)
    }
}
pub struct NativeTaskFactory {
    tasks: Arc<NativeTasks>,
}
impl NativeTaskFactory {
    pub fn new(tasks: Arc<NativeTasks>) -> Self {
        Self { tasks }
    }
}
impl QueryFactory for NativeTaskFactory {
    fn create(&self, key: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        let request = self
            .tasks
            .requests
            .get(&id(key))
            .ok_or_else(|| self.tasks.failure("unregistered native task"))?
            .clone();
        self.tasks.created.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(NativeTask {
            tasks: self.tasks.clone(),
            request,
            terminal: None,
        }))
    }
}
struct NativeTask {
    tasks: Arc<NativeTasks>,
    request: Arc<Request>,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for NativeTask {
    fn id(&self) -> QueryId {
        self.request.key
    }
    fn weight(&self) -> MemoryWeight {
        // The immutable request and completed products retain their snapshot
        // reservations. Only these Arc handles and terminal state are local.
        MemoryWeight {
            retained: std::mem::size_of::<Self>() as u64,
            ..self.tasks.weight
        }
    }
    fn continuation_bytes(&self) -> u64 {
        std::mem::size_of::<Self>() as u64
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if let Some(result) = self.terminal {
            return match result {
                Ok(p) => QueryPoll::Complete(p),
                Err(e) => QueryPoll::Failed(e),
            };
        }
        let mut waiting = Vec::new();
        for (key, record) in &self.request.reads {
            match cx.read(*key) {
                Poll::Pending => waiting.push(*key),
                Poll::Ready(Err(error)) => {
                    self.terminal = Some(Err(error));
                    return QueryPoll::Failed(error);
                }
                Poll::Ready(Ok(product))
                    if product.address == key.address && product.record == *record => {}
                Poll::Ready(Ok(_)) => {
                    let error = self
                        .tasks
                        .failure("native dependency output changed within the immutable snapshot");
                    self.terminal = Some(Err(error));
                    return QueryPoll::Failed(error);
                }
            }
        }
        if let Some(key) = self.request.guard {
            match cx.read(key) {
                Poll::Pending => waiting.push(key),
                Poll::Ready(Err(error)) => {
                    self.terminal = Some(Err(error));
                    return QueryPoll::Failed(error);
                }
                Poll::Ready(Ok(product))
                    if self
                        .tasks
                        .snapshot
                        .guard
                        .get()
                        .is_some_and(|guard| guard.accepts(product)) =>
                {
                    ()
                }
                Poll::Ready(Ok(_)) => {
                    let error = self
                        .tasks
                        .failure("Ready seal-demand output differs from its settled dependency");
                    self.terminal = Some(Err(error));
                    return QueryPoll::Failed(error);
                }
            }
        }
        if !waiting.is_empty() {
            return QueryPoll::Waiting(waiting);
        }
        let mut products = DemandProducts::new(self.tasks.snapshot.clone());
        let result = match &self.request.work {
            Work::Instance(key) => products.instance(key).map(|p| p.product),
            Work::Arena(key) => products.arena_summary(key).map(|p| p.product),
            Work::Ready(key) => products.ready(key).map(|p| p.product),
        }
        .map_err(|error| self.tasks.failure(&error));
        self.tasks.ready_transformations.fetch_add(
            products.local.queries.counts.transformations,
            Ordering::Relaxed,
        );
        self.terminal = Some(result);
        match result {
            Ok(p) => QueryPoll::Complete(p),
            Err(e) => QueryPoll::Failed(e),
        }
    }
}

/// Source-led Codegen tasks sharing this exact native snapshot. Link/root
/// manifests are a separate producer and are deliberately not registered here.
pub struct CodegenTasks {
    native: Arc<NativeTasks>,
    requests: BTreeMap<Id, CodegenKey>,
    whole: BTreeMap<Id, crate::native::object_queries::whole::WholeCodegenKey>,
    weight: MemoryWeight,
}
impl CodegenTasks {
    pub fn new(
        native: Arc<NativeTasks>,
        keys: Vec<CodegenKey>,
        weight: MemoryWeight,
    ) -> Result<Arc<Self>, String> {
        Self::with_whole_modules(native, keys, vec![], weight)
    }
    pub fn with_whole_modules(
        native: Arc<NativeTasks>,
        keys: Vec<CodegenKey>,
        whole: Vec<crate::native::object_queries::whole::WholeCodegenKey>,
        weight: MemoryWeight,
    ) -> Result<Arc<Self>, String> {
        let mut requests = BTreeMap::new();
        for key in keys {
            if !native
                .snapshot
                .owners
                .contains_key(&key.owner.ready_query_id().address.0)
                || requests.insert(id(key.query_id()), key).is_some()
            {
                return Err("unknown or duplicate Codegen request".into());
            }
        }
        let mut whole_requests = BTreeMap::new();
        for key in whole {
            if key
                .retained_bytes()
                .saturating_add(std::mem::size_of::<ObjectTaskRequest>() as u64)
                > weight.retained
            {
                return Err("whole-module Codegen exceeds retained allowance".into());
            }
            if key.units().iter().any(|unit| {
                !native
                    .snapshot
                    .owners
                    .contains_key(&unit.owner.ready_query_id().address.0)
            }) || whole_requests.insert(id(key.query_id()), key).is_some()
            {
                return Err("unknown or duplicate whole-module Codegen request".into());
            }
        }
        Ok(Arc::new(Self {
            native,
            requests,
            whole: whole_requests,
            weight,
        }))
    }
}
fn no_link(
    _: &LinkKey,
    _: &LinkInputs,
    _: &[hirsel_lir::stage::ObjectArtifact],
) -> Result<Vec<u8>, String> {
    Err("Link is not registered in the Codegen snapshot".into())
}
impl ObjectTaskSnapshot for CodegenTasks {
    fn store(&self) -> &dyn ResultStore {
        self.native.snapshot.store.as_ref()
    }
    fn request(&self, key: QueryId) -> Result<ObjectTaskRequest, Reference> {
        if let Some(key) = self.whole.get(&id(key)) {
            return Ok(ObjectTaskRequest::WholeCodegen {
                key: key.clone(),
                weight: self.weight,
            });
        }
        let key = self
            .requests
            .get(&id(key))
            .ok_or_else(|| self.failure("unregistered Codegen task"))?;
        Ok(ObjectTaskRequest::Codegen {
            key: key.clone(),
            ready: key.owner.ready_query_id(),
            weight: self.weight,
        })
    }
    fn ready_descriptor(&self, product: ProductRef) -> Result<ReadyProduct, Reference> {
        ready_product(self.store(), product).map_err(|e| self.failure(&e))
    }
    fn optimizer_query(&self, product: ProductRef) -> Result<QueryId, Reference> {
        self.native
            .snapshot
            .owners
            .contains_key(&product.address.0)
            .then_some(QueryId {
                kind: READY_KIND,
                address: product.address,
            })
            .ok_or_else(|| self.failure("unregistered Ready optimizer dependency"))
    }
    fn optimizer_queries(
        &self,
        key: &CodegenKey,
        ready: &ReadyProduct,
    ) -> Result<Vec<(QueryId, Option<ProductRef>)>, Reference> {
        if !ready.optimizer_reads.is_empty() {
            return Err(self.failure("unregistered Ready transformation reads"));
        }
        self.native
            .snapshot
            .codegen_owners(&key.owner, &key.backend)
            .map(|owners| {
                owners
                    .into_iter()
                    .skip(1)
                    .map(|owner| (owner.ready_query_id(), None))
                    .collect()
            })
            .map_err(|error| self.failure(&error))
    }
    fn link_dependencies(&self, _: ProductRef) -> Result<LinkDependencies, Reference> {
        Err(self.failure("native Link manifest is not registered"))
    }
    fn link_input(&self, _: ProductRef) -> Result<Reference, Reference> {
        Err(self.failure("native Link input is not registered"))
    }
    fn products(&self, settled: &[ProductRef]) -> Result<Box<dyn ObjectProducts>, Reference> {
        let native = SettledProducts::new(self.native.snapshot.clone(), settled)
            .map_err(|e| self.failure(&e))?;
        Ok(Box::new(
            ObjectQueries::new(self.native.snapshot.store.clone(), native, no_link)
                .with_verification(self.native.snapshot.verify_store),
        ))
    }
    fn failure(&self, message: &str) -> Reference {
        self.native.failure(message)
    }
}
