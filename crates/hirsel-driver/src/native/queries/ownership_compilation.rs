//! Source-selected diagnostic compilation. Instance discovery and arena
//! settlement precede each explicit observed Ready dependency. No ordinary
//! object or Link task is needed to obtain a checked ownership plan.
pub(super) mod metadata;
use super::{
    arena_tasks::ArenaTasks,
    instance_tasks::InstanceTasks,
    ownership_tasks::{OWNERSHIP_READY_KIND, OWNERSHIP_ROOT_KIND, OwnershipTasks},
    products::{EmissionOptions, NativeSnapshot},
    tasks::NativeTasks,
    *,
};
use hirsel_engine::{
    runtime::*,
    store::{Action, Address, Domain},
};
use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
    task::Poll,
};

pub const OWNERSHIP_COMPILATION_KIND: u32 = 0x4e00_002d;
struct Dispatch {
    discovery: ProductRef,
    arena: ProductRef,
    ownership: Arc<OwnershipTasks>,
    factories: BTreeMap<u32, Arc<dyn QueryFactory>>,
}
pub struct OwnershipCompilation {
    instances: Arc<InstanceTasks>,
    arena: Arc<ArenaTasks>,
    emission: EmissionOptions,
    selection: hirsel_lir::ownership_trace::Selection,
    query: QueryId,
    weight: MemoryWeight,
    allowance: Mutex<Option<MemoryReservation>>,
    verify_store: hirsel_engine::store::VerifyStore,
    dispatch: OnceLock<Dispatch>,
    completed: OnceLock<(ProductRef, ProductRef)>,
}
impl OwnershipCompilation {
    pub fn new(
        instances: Arc<InstanceTasks>,
        arena: Arc<ArenaTasks>,
        emission: EmissionOptions,
        weight: MemoryWeight,
        allowance: MemoryReservation,
        verify_store: hirsel_engine::store::VerifyStore,
    ) -> Result<Arc<Self>, String> {
        Self::new_selected(
            instances,
            arena,
            hirsel_lir::ownership_trace::Selection::Resources,
            emission,
            weight,
            allowance,
            verify_store,
        )
    }
    pub fn new_selected(
        instances: Arc<InstanceTasks>,
        arena: Arc<ArenaTasks>,
        selection: hirsel_lir::ownership_trace::Selection,
        emission: EmissionOptions,
        weight: MemoryWeight,
        allowance: MemoryReservation,
        verify_store: hirsel_engine::store::VerifyStore,
    ) -> Result<Arc<Self>, String> {
        let mut query = QueryId {
            kind: OWNERSHIP_COMPILATION_KIND,
            address: Address(ContentHash::of_parts(
                "hirsel.native.ownership-compilation.v1",
                [
                    instances.discovery_id().address.0.0.as_slice(),
                    arena.query_id().address.0.0.as_slice(),
                    emission.encode()?.as_slice(),
                ],
            )),
        };
        if selection == hirsel_lir::ownership_trace::Selection::Owned {
            query.address = Address(ContentHash::of_parts(
                "hirsel.native.ownership-compilation.owned.v1",
                [query.address.0.0.as_slice()],
            ));
        }
        if weight.retained < std::mem::size_of::<Task>() as u64 {
            return Err("ownership compilation continuation exceeds admission".into());
        }
        Ok(Arc::new(Self {
            verify_store,
            instances,
            arena,
            emission,
            selection,
            query,
            weight,
            allowance: Mutex::new(Some(allowance)),
            dispatch: OnceLock::new(),
            completed: OnceLock::new(),
        }))
    }
    /// Reopen the bounded diagnostic of an actual native dependency.
    pub fn failure(&self, reference: Reference) -> Option<String> {
        self.instances.failure_message(reference)
    }
    pub fn query_id(&self) -> QueryId {
        self.query
    }
    pub fn factories(self: &Arc<Self>) -> Vec<(u32, Arc<dyn QueryFactory>)> {
        let instances: Arc<dyn QueryFactory> = Arc::new(
            super::instance_tasks::InstanceTaskFactory::new(self.instances.clone()),
        );
        let arena: Arc<dyn QueryFactory> = Arc::new(super::arena_tasks::ArenaTaskFactory::new(
            self.arena.clone(),
        ));
        let own: Arc<dyn QueryFactory> = Arc::new(Factory(self.clone()));
        let mut factories = Vec::new();
        for kind in [
            INSTANCE_KIND,
            super::instance::CALLABLE_VALUE_KIND,
            super::instance_tasks::CALLABLE_GRAPH_KIND,
            super::instance_tasks::INSTANCE_INPUT_KIND,
            super::instance_tasks::DISCOVERY_KIND,
        ] {
            factories.push((kind, instances.clone()));
        }
        for kind in [
            ARENA_CANDIDATES_KIND,
            ARENA_CHOSEN_KIND,
            super::arena_tasks::ARENA_GRAPH_KIND,
        ] {
            factories.push((kind, arena.clone()));
        }
        for kind in [
            OWNERSHIP_COMPILATION_KIND,
            OWNERSHIP_READY_KIND,
            OWNERSHIP_ROOT_KIND,
            super::seal_demand::SEAL_DEMAND_KIND,
        ] {
            factories.push((kind, own.clone()));
        }
        factories
    }
    fn initialize(&self, discovery: ProductRef, arena: ProductRef) -> Result<&Dispatch, String> {
        if let Some(dispatch) = self.dispatch.get() {
            return if dispatch.discovery == discovery && dispatch.arena == arena {
                Ok(dispatch)
            } else {
                Err("ownership compilation dependencies changed".into())
            };
        }
        let mut operations = self.instances.source_operations(discovery.clone())?;
        let operations_bytes = metadata::operations(&mut operations);
        let mut snapshot = NativeSnapshot::new(
            self.instances.selected_store().clone(),
            self.instances.products(discovery.clone())?,
            self.arena.graph(arena.clone())?,
            self.instances.options().clone(),
            self.emission.clone(),
            self.instances.source_inputs().declarations.manifest()?,
            Arc::new(operations),
        )?
        .with_store_verification(self.verify_store)?;
        Arc::get_mut(&mut snapshot)
            .ok_or("ownership snapshot shared before admission")?
            .retained_allowances = Vec::with_capacity(5);
        Arc::get_mut(&mut snapshot)
            .unwrap()
            .retained_allowances
            .extend([
                self.instances.retained_allowance(),
                self.instances.retained_source_allowance(),
                self.arena.retained_allowance(),
            ]);
        let mut allowance = self
            .allowance
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .ok_or("ownership dispatch admission consumed twice")?;
        if snapshot.needs_guard {
            let inputs = snapshot.instances.completed_inputs()?;
            let bytes = super::seal_demand::Plan::admission_bytes(
                inputs
                    .iter()
                    .map(|(_, output)| &output.facts.body.seal_demand),
            );
            let guard_allowance = allowance
                .split(bytes)
                .ok_or("ownership seal-demand metadata exceeds dispatch admission")?;
            let snapshot = Arc::get_mut(&mut snapshot).unwrap();
            snapshot.guard_admitted_bytes = bytes;
            snapshot.retained_allowances.push(Arc::new(guard_allowance));
        }
        let snapshot_bytes = Arc::get_mut(&mut snapshot)
            .unwrap()
            .retained_dispatch_bytes();
        let mut native = NativeTasks::for_ownership(snapshot, self.weight)?;
        let dispatch_bytes = Arc::get_mut(&mut native).unwrap().retained_dispatch_bytes();
        let mut copied = hirsel_engine::NativeMetadataWeight::default();
        copied.add(size_of::<Self>() + 2 * size_of::<usize>());
        metadata::emission(&mut copied, &self.emission);
        copied.add(size_of::<MemoryReservation>() + 2 * size_of::<usize>());
        copied
            .add(size_of::<super::instance_tasks::InstanceTaskFactory>() + 2 * size_of::<usize>());
        copied.add(size_of::<super::arena_tasks::ArenaTaskFactory>() + 2 * size_of::<usize>());
        copied.add(size_of::<Factory>() + 2 * size_of::<usize>());
        copied.add(size_of::<super::ownership_tasks::Factory>() + 2 * size_of::<usize>());
        copied.bulk_tree::<u32, Arc<dyn QueryFactory>>(
            2 + usize::from(native.guard_factory().is_some()),
        );
        let bytes = operations_bytes
            .saturating_add(snapshot_bytes)
            .saturating_add(dispatch_bytes)
            .saturating_add(copied.bytes());

        let dispatch_allowance = allowance
            .split(bytes)
            .ok_or("ownership compilation dispatch allowance exhausted")?;
        // NativeTasks uniquely owns the snapshot until diagnostic factories are
        // published. Keep each producer's reservation through the final scope.
        Arc::get_mut(&mut Arc::get_mut(&mut native).unwrap().snapshot)
            .unwrap()
            .retained_allowances
            .push(Arc::new(dispatch_allowance));
        let guard = native.guard_factory();
        let ownership =
            OwnershipTasks::new_selected(native, self.selection, self.weight, allowance)?;
        let factory = ownership.factory();
        let mut factories = BTreeMap::from([
            (OWNERSHIP_READY_KIND, factory.clone()),
            (OWNERSHIP_ROOT_KIND, factory),
        ]);
        if let Some((query, factory)) = guard {
            factories.insert(query.kind, factory);
        }
        self.dispatch
            .set(Dispatch {
                discovery,
                arena,
                ownership,
                factories,
            })
            .map_err(|_| "duplicate ownership compilation dispatch")?;
        Ok(self.dispatch.get().unwrap())
    }
    pub fn outcome(
        &self,
        selected: ProductRef,
    ) -> Result<(Arc<OwnershipTasks>, ProductRef), String> {
        let (completion, root) = self
            .completed
            .get()
            .ok_or("ownership compilation is incomplete")?;
        if *completion != selected {
            return Err("ownership compilation completion differs".into());
        }
        let ownership = self
            .dispatch
            .get()
            .ok_or("ownership dispatch is unavailable")?
            .ownership
            .clone();
        ownership.outputs(root.clone())?;
        Ok((ownership, root.clone()))
    }
    fn publish(&self, root: ProductRef) -> Result<ProductRef, String> {
        let dispatch = self
            .dispatch
            .get()
            .ok_or("ownership dispatch is unavailable")?;
        dispatch.ownership.outputs(root.clone())?;
        let bytes = serde_json::to_vec(&(
            root.address.0,
            root.action.0,
            root.archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                .map_err(|e| e.to_string())?
                .reference(),
        ))
        .map_err(|e| e.to_string())?;
        let domain = Domain {
            kind: OWNERSHIP_COMPILATION_KIND,
            schema: 1,
        };
        let store = self.instances.selected_store();
        let record = store
            .put_content(domain, &bytes)
            .ok_or("ownership compilation transport exhausted")?;
        if domain.reference(&bytes) != Some(record) {
            return Err("ownership compilation content differs".into());
        }
        let action = Action(ContentHash::of_parts(
            "hirsel.native.ownership-compilation-action.v1",
            [
                self.query.address.0.0.as_slice(),
                record.output.0.0.as_slice(),
            ],
        ));
        let product = ProductRef::import_archive(
            self.query.address,
            action,
            store
                .admit_product_archive(record, bytes.into())
                .ok_or("native product archive is unavailable or exceeds admission")?,
        );
        store.associate_product(product.address, action, record);
        self.completed
            .set((product.clone(), root))
            .map_err(|_| "ownership compilation completed twice")?;
        Ok(product)
    }
}
struct Factory(Arc<OwnershipCompilation>);
impl QueryFactory for Factory {
    fn create(&self, query: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if query == self.0.query {
            return Ok(Box::new(Task {
                owner: self.0.clone(),
                terminal: None,
            }));
        }
        self.0
            .dispatch
            .get()
            .and_then(|dispatch| dispatch.factories.get(&query.kind))
            .ok_or_else(|| {
                self.0
                    .instances
                    .failure("unregistered ownership compilation task")
            })?
            .create(query)
    }
}
struct Task {
    owner: Arc<OwnershipCompilation>,
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
        if self.terminal.is_none() {
            let fail = |error: String| self.owner.instances.failure(&error);
            let mut selected = Vec::new();
            let mut waiting = Vec::new();
            for query in [
                self.owner.instances.discovery_id(),
                self.owner.arena.query_id(),
            ] {
                match cx.read(query) {
                    Poll::Pending => waiting.push(query),
                    Poll::Ready(Ok(product)) if product.address == query.address => {
                        selected.push(product)
                    }
                    Poll::Ready(Err(error)) => {
                        self.terminal = Some(Err(error));
                        break;
                    }
                    _ => {
                        self.terminal = Some(Err(fail(
                            "ownership dependency returned another address".into(),
                        )));
                        break;
                    }
                }
            }
            if self.terminal.is_none() {
                if !waiting.is_empty() {
                    return QueryPoll::Waiting(waiting);
                }
                self.terminal = match self
                    .owner
                    .initialize(selected[0].clone(), selected[1].clone())
                {
                    Err(error) => Some(Err(fail(error))),
                    Ok(dispatch) => match cx.read(dispatch.ownership.query_id()) {
                        Poll::Pending => {
                            return QueryPoll::Waiting(vec![dispatch.ownership.query_id()]);
                        }
                        Poll::Ready(Err(error)) => Some(Err(error)),
                        Poll::Ready(Ok(root)) => Some(self.owner.publish(root).map_err(fail)),
                    },
                };
            }
        }
        match self.terminal.clone().unwrap() {
            Ok(product) => QueryPoll::Complete(product),
            Err(error) => QueryPoll::Failed(error),
        }
    }
}
