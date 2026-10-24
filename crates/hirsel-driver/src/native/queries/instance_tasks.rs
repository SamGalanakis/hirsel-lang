//! Cold native instance continuations. The shared registry stores only request
//! and completed descriptor metadata; each poll owns its importer and source
//! reader. No source/native work runs under its lock or in factory creation.
use super::{
    demand::{DemandInstances, DiscoveredInstance, Request},
    instance::InstanceQueries,
    source_inputs::NativeInstanceInputs,
    *,
};
use hirsel_artifact_store::{Action, Address, Domain};
use hirsel_engine::{runtime::*, store::ResultStore};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    task::Poll,
};

pub const INSTANCE_INPUT_KIND: u32 = 0x4e00_0020;
pub const DISCOVERY_KIND: u32 = 0x4e00_0021;
const FAILURE: Domain = Domain {
    kind: 0x4e00_001e,
    schema: 1,
};
type Index = BTreeMap<ContentHash, Arc<Registered>>;
struct Registered {
    key: InstanceAddress,
    request: Request,
    input: OnceLock<ProductRef>,
    output: OnceLock<Arc<DiscoveredInstance>>,
}
struct Registry {
    entries: Index,
    inputs: Index,
    charged: u64,
}
/// Source production must already have selected actual current V envelopes.
/// Cold source production under VerifyOff/recomputation remains a separate
/// source task; these native tasks never infer a missing source observation.
pub struct InstanceTasks {
    source: Arc<NativeInstanceInputs>,
    store: Arc<dyn ResultStore>,
    options: crate::native::Options,
    policy: ContentHash,
    native: Arc<hirsel_lir::validate::NativeCache>,
    registry: Mutex<Registry>,
    roots: Vec<InstanceAddress>,
    discovery: QueryId,
    completed: OnceLock<ProductRef>,
    weight: MemoryWeight,
    allowance: Arc<MemoryReservation>,
    fallback: Reference,
    pub imports: AtomicU64,
    pub recipe_decodes: AtomicU64,
    pub source_body_demands: AtomicU64,
}
fn instance_id(key: &InstanceAddress) -> QueryId {
    QueryId {
        kind: INSTANCE_KIND,
        address: key.address(INSTANCE_KIND),
    }
}
fn input_id(key: &InstanceAddress) -> QueryId {
    QueryId {
        kind: INSTANCE_INPUT_KIND,
        address: key.address(INSTANCE_INPUT_KIND),
    }
}
impl InstanceTasks {
    pub fn new(
        source: Arc<NativeInstanceInputs>,
        store: Arc<dyn ResultStore>,
        options: crate::native::Options,
        policy: ContentHash,
        weight: MemoryWeight,
        allowance: MemoryReservation,
    ) -> Result<Arc<Self>, String> {
        let provider = source.provider();
        let native = InstanceQueries::validator(&store);
        let mut initial = DemandInstances::with_validator(
            &provider,
            store.clone(),
            options.target.clone(),
            policy,
            native.clone(),
        );
        let roots = initial.roots_from(&provider)?;
        let selection = serde_json::to_vec(
            &roots
                .iter()
                .map(|key| key.address(INSTANCE_KIND).0)
                .collect::<Vec<_>>(),
        )
        .map_err(|e| e.to_string())?;
        let discovery = QueryId {
            kind: DISCOVERY_KIND,
            address: Address(ContentHash::of_parts(
                "hirsel.native.discovery-address.v1",
                [selection.as_slice()],
            )),
        };
        let fallback = store
            .put_content(
                FAILURE,
                b"native instance task resource or protocol refusal",
            )
            .ok_or("native task failure publication failed")?;
        let root_bytes = roots
            .iter()
            .map(|key| key.instance.encoding().len() + 512)
            .sum::<usize>();
        if weight.retained < root_bytes as u64 + std::mem::size_of::<DiscoveryTask>() as u64
            || weight.retained < std::mem::size_of::<InstanceTask>() as u64
            || weight.retained < std::mem::size_of::<DiscoveryTask>() as u64
        {
            return Err("native task frame exceeds retained allowance".into());
        }
        let tasks = Arc::new(Self {
            source,
            store,
            options,
            policy,
            native,
            roots,
            discovery,
            registry: Mutex::new(Registry {
                entries: BTreeMap::new(),
                inputs: BTreeMap::new(),
                charged: 0,
            }),
            completed: OnceLock::new(),
            weight,
            allowance: Arc::new(allowance),
            fallback,
            imports: AtomicU64::new(0),
            recipe_decodes: AtomicU64::new(0),
            source_body_demands: AtomicU64::new(0),
        });
        tasks.register(initial.registered_requests()?)?;
        Ok(tasks)
    }
    pub(super) fn retained_allowance(&self) -> Arc<MemoryReservation> {
        self.allowance.clone()
    }
    fn producer(&self) -> DemandInstances {
        DemandInstances::with_validator(
            &self.source.provider(),
            self.store.clone(),
            self.options.target.clone(),
            self.policy,
            self.native.clone(),
        )
    }
    fn register(&self, requests: Vec<(InstanceAddress, Request)>) -> Result<(), String> {
        // Encoding/validation runs outside the index lock. admit() has already
        // checked recipe domains and presence through the sole demand core.
        let entries = requests
            .into_iter()
            .map(|(key, request)| {
                let bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
                let charge = 4096u64.saturating_add((bytes.len() as u64).saturating_mul(4));
                Ok((
                    key.address(INSTANCE_KIND).0,
                    key.address(INSTANCE_INPUT_KIND).0,
                    charge,
                    Arc::new(Registered {
                        key,
                        request,
                        input: OnceLock::new(),
                        output: OnceLock::new(),
                    }),
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        for (id, input, charge, entry) in entries {
            if let Some(previous) = registry.entries.get(&id) {
                if previous.key != entry.key || previous.request != entry.request {
                    return Err("inconsistent native request registration".into());
                }
                continue;
            }
            if registry.entries.len() >= hirsel_rc::instance::MAX_INSTANCES
                || charge > self.allowance.bytes().saturating_sub(registry.charged)
            {
                return Err("native discovery metadata allowance exhausted".into());
            }
            registry.charged += charge;
            registry.inputs.insert(input, entry.clone());
            registry.entries.insert(id, entry);
        }
        Ok(())
    }
    fn registered(&self, key: &InstanceAddress) -> Result<Arc<Registered>, String> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .get(&key.address(INSTANCE_KIND).0)
            .cloned()
            .ok_or("unregistered native instance".into())
    }
    fn entries(&self) -> Vec<Arc<Registered>> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .values()
            .cloned()
            .collect()
    }
    fn charge_output(&self, output: &DiscoveredInstance) -> Result<(), String> {
        let ordinary = serde_json::to_vec(&(
            &output.callees,
            &output.selection_reads,
            &output.constant_reads,
            &output.templates,
            &output.operation_freedom,
            &output.emission,
        ))
        .map_err(|e| e.to_string())?;
        let bytes = ordinary
            .len()
            .saturating_add(output.ready_source.encode()?.len())
            .saturating_add(
                output
                    .dependencies
                    .iter()
                    .map(|key| key.instance.encoding().len() + 256)
                    .sum(),
            );
        let charge = 4096u64.saturating_add((bytes as u64).saturating_mul(4));
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        if charge > self.allowance.bytes().saturating_sub(registry.charged) {
            return Err("native completed descriptor allowance exhausted".into());
        }
        registry.charged += charge;
        Ok(())
    }
    fn put(&self, key: QueryId, bytes: &[u8]) -> Result<ProductRef, String> {
        let domain = Domain {
            kind: key.kind,
            schema: 1,
        };
        let record = self
            .store
            .put_content(domain, bytes)
            .ok_or("native task output transport failed")?;
        if domain.reference(bytes) != Some(record) {
            return Err("native task output content identity differs".into());
        }
        let action = Action(ContentHash::of_parts(
            "hirsel.native.task-output-action.v1",
            [key.address.0.0.as_slice(), record.output.0.0.as_slice()],
        ));
        self.store.associate_product(key.address, action, record);
        Ok(ProductRef {
            address: key.address,
            action,
            record,
        })
    }
    pub fn failure(&self, message: &str) -> Reference {
        self.store
            .put_content(FAILURE, message.as_bytes())
            .unwrap_or(self.fallback)
    }
    pub(super) fn source_inputs(&self) -> &NativeInstanceInputs {
        &self.source
    }
    pub(super) fn options(&self) -> &crate::native::Options {
        &self.options
    }
    pub(super) fn policy(&self) -> ContentHash {
        self.policy
    }
    pub(super) fn selected_store(&self) -> &Arc<dyn ResultStore> {
        &self.store
    }
    pub fn discovery_id(&self) -> QueryId {
        self.discovery
    }
    pub fn roots(&self) -> &[InstanceAddress] {
        &self.roots
    }
    /// Snapshot assembly reads only the actual settled discovery output. No
    /// dependency/recipe/Value work is hidden here; consumers independently
    /// re-admit C using the shared snapshot-private native cache.
    pub fn products(&self, settled: ProductRef) -> Result<DemandInstances, String> {
        if self.completed.get() != Some(&settled) {
            return Err("native discovery is not settled at this output".into());
        }
        let mut products = self.producer();
        let entries = self.entries();
        for entry in &entries {
            products.admit(entry.request.clone())?;
        }
        for entry in entries {
            let output = entry
                .output
                .get()
                .ok_or("native discovery has an unfinished instance")?;
            products.attach_completed(&entry.key, output.clone())?;
        }
        Ok(products)
    }
    pub fn completed_instances(
        &self,
        settled: ProductRef,
    ) -> Result<Vec<(InstanceAddress, Arc<DiscoveredInstance>)>, String> {
        if self.completed.get() != Some(&settled) {
            return Err("native discovery has not completed".into());
        }
        let mut outputs = self
            .entries()
            .into_iter()
            .map(|entry| {
                let output = entry
                    .output
                    .get()
                    .ok_or("native instance has not completed")?
                    .clone();
                Ok((entry.key.clone(), output))
            })
            .collect::<Result<Vec<_>, String>>()?;
        outputs.sort_by(|a, b| a.0.canonical_cmp(&b.0));
        Ok(outputs)
    }
}

pub struct InstanceTaskFactory {
    tasks: Arc<InstanceTasks>,
}
impl InstanceTaskFactory {
    pub fn new(tasks: Arc<InstanceTasks>) -> Self {
        Self { tasks }
    }
}
impl QueryFactory for InstanceTaskFactory {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if id == self.tasks.discovery {
            return Ok(Box::new(DiscoveryTask {
                tasks: self.tasks.clone(),
                frontier: self.tasks.roots.clone(),
                seen: BTreeSet::new(),
                terminal: None,
            }));
        }
        let entry = {
            let registry = self
                .tasks
                .registry
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let index = match id.kind {
                INSTANCE_KIND => &registry.entries,
                INSTANCE_INPUT_KIND => &registry.inputs,
                _ => return Err(self.tasks.fallback),
            };
            index
                .get(&id.address.0)
                .cloned()
                .ok_or(self.tasks.fallback)?
        };
        Ok(Box::new(InstanceTask {
            tasks: self.tasks.clone(),
            entry,
            id,
            terminal: None,
        }))
    }
}
struct InstanceTask {
    tasks: Arc<InstanceTasks>,
    entry: Arc<Registered>,
    id: QueryId,
    terminal: Option<Result<ProductRef, Reference>>,
}
fn finish(result: Result<ProductRef, Reference>) -> QueryPoll {
    match result {
        Ok(product) => QueryPoll::Complete(product),
        Err(error) => QueryPoll::Failed(error),
    }
}
impl QueryTask for InstanceTask {
    fn id(&self) -> QueryId {
        self.id
    }
    fn weight(&self) -> MemoryWeight {
        self.tasks.weight
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if let Some(result) = self.terminal {
            return finish(result);
        }
        if self.id.kind == INSTANCE_KIND {
            match cx.read(input_id(&self.entry.key)) {
                Poll::Pending => return QueryPoll::Waiting(vec![input_id(&self.entry.key)]),
                Poll::Ready(Err(error)) => {
                    self.terminal = Some(Err(error));
                    return finish(Err(error));
                }
                Poll::Ready(Ok(product)) if self.entry.input.get() == Some(&product) => {}
                _ => {
                    let error = self
                        .tasks
                        .failure("native instance input differs from selected snapshot");
                    self.terminal = Some(Err(error));
                    return finish(Err(error));
                }
            }
        }
        let result = (|| {
            let mut producer = self.tasks.producer();
            producer.admit(self.entry.request.clone())?;
            let mut source = self.tasks.source.provider();
            if self.id.kind == INSTANCE_INPUT_KIND {
                let bytes =
                    producer.producer_inputs(&source, &self.entry.key, &self.tasks.options)?;
                let product = self.tasks.put(self.id, &bytes)?;
                if self.entry.input.set(product).is_err()
                    && self.entry.input.get() != Some(&product)
                {
                    return Err("native selected input completion changed".into());
                }
                return Ok(product);
            }
            let output =
                producer.instance_from(&mut source, &self.entry.key, &self.tasks.options)?;
            self.tasks
                .imports
                .fetch_add(producer.imports, Ordering::Relaxed);
            self.tasks
                .recipe_decodes
                .fetch_add(producer.recipe_decodes, Ordering::Relaxed);
            self.tasks
                .source_body_demands
                .fetch_add(source.body_demands, Ordering::Relaxed);
            self.tasks.register(producer.registered_requests()?)?;
            self.tasks.charge_output(&output)?;
            let product = output.product.product;
            if let Err(other) = self.entry.output.set(output)
                && self.entry.output.get().unwrap().product.product != other.product.product
            {
                return Err("native instance completion changed".into());
            }
            Ok(product)
        })()
        .map_err(|error: String| self.tasks.failure(&error));
        self.terminal = Some(result);
        finish(result)
    }
}
struct DiscoveryTask {
    tasks: Arc<InstanceTasks>,
    frontier: Vec<InstanceAddress>,
    seen: BTreeSet<ContentHash>,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for DiscoveryTask {
    fn id(&self) -> QueryId {
        self.tasks.discovery
    }
    fn weight(&self) -> MemoryWeight {
        self.tasks.weight
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if let Some(result) = self.terminal {
            return finish(result);
        }
        let result = self.advance(cx);
        match result {
            Ok(Some(product)) => {
                self.terminal = Some(Ok(product));
                finish(Ok(product))
            }
            Ok(None) => QueryPoll::Waiting(self.frontier.iter().map(instance_id).collect()),
            Err(error) => {
                self.terminal = Some(Err(error));
                finish(Err(error))
            }
        }
    }
}
impl DiscoveryTask {
    fn advance(&mut self, cx: &mut dyn QueryContext) -> Result<Option<ProductRef>, Reference> {
        let mut frontier = std::mem::take(&mut self.frontier);
        let mut read = BTreeSet::new();
        while let Some(key) = frontier.pop() {
            let address = key.address(INSTANCE_KIND).0;
            if self.seen.contains(&address) || !read.insert(address) {
                continue;
            }
            match cx.read(instance_id(&key)) {
                Poll::Pending => self.frontier.push(key),
                Poll::Ready(Err(error)) => return Err(error),
                Poll::Ready(Ok(product)) => {
                    let entry = self
                        .tasks
                        .registered(&key)
                        .map_err(|e| self.tasks.failure(&e))?;
                    let output = entry.output.get().ok_or_else(|| {
                        self.tasks
                            .failure("native task returned no completed descriptor")
                    })?;
                    if output.product.product != product {
                        return Err(self.tasks.failure("native discovery read another output"));
                    }
                    self.seen.insert(address);
                    frontier.extend(output.dependencies.iter().cloned());
                }
            }
            let bytes = frontier
                .iter()
                .chain(&self.frontier)
                .map(|key| key.instance.encoding().len() + 512)
                .sum::<usize>()
                .saturating_add(self.seen.len().saturating_mul(128))
                .saturating_add(read.len().saturating_mul(128));
            if bytes as u64 > self.tasks.weight.retained {
                return Err(self
                    .tasks
                    .failure("native discovery frame exceeds retained allowance"));
            }
        }
        self.frontier.sort_by(InstanceAddress::canonical_cmp);
        if !self.frontier.is_empty() {
            return Ok(None);
        }
        let result = (|| {
            let mut rows = self
                .tasks
                .entries()
                .into_iter()
                .map(|entry| {
                    let product = entry
                        .output
                        .get()
                        .ok_or("discovery output is unfinished")?
                        .product
                        .product;
                    Ok((
                        entry.key.instance.encoding().to_vec(),
                        product.address.0,
                        product.record,
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            let bytes = serde_json::to_vec(&rows).map_err(|e| e.to_string())?;
            let product = self.tasks.put(self.tasks.discovery, &bytes)?;
            if self.tasks.completed.set(product).is_err()
                && self.tasks.completed.get() != Some(&product)
            {
                return Err("native discovery completion changed".into());
            }
            Ok(Some(product))
        })();
        result.map_err(|e: String| self.tasks.failure(&e))
    }
}
