//! Cold native instance continuations. The shared registry stores only request
//! and completed descriptor metadata; each poll owns its importer and source
//! reader. No source/native work runs under its lock or in factory creation.
use super::{
    demand::{DemandInstances, DiscoveredInstance, Request},
    instance::InstanceQueries,
    source_inputs::{NativeFrontLower, NativeInstanceInputs, NativeSourceProvider},
    *,
};
use hirsel_engine::store::{Action, Address, Domain};
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
    source: OnceLock<(ProductRef, hirsel_engine::NativeOriginalProduct)>,
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
    front: Option<NativeFrontLower>,
    store: Arc<dyn ResultStore>,
    options: crate::native::Options,
    verify_store: hirsel_engine::store::VerifyStore,
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
        mut allowance: MemoryReservation,
    ) -> Result<Arc<Self>, String> {
        source.retain_metadata(&mut allowance)?;
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
            front: None,
            store,
            options,
            verify_store: hirsel_engine::store::VerifyStore::Off,
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
    /// Bind explicit host cache verification before this registry is shared
    /// with factories. Every worker-local Instance producer receives the policy.
    pub fn with_store_verification(
        mut self: Arc<Self>,
        policy: hirsel_engine::store::VerifyStore,
    ) -> Result<Arc<Self>, String> {
        let tasks = Arc::get_mut(&mut self)
            .ok_or("native tasks were shared before store verification binding")?;
        tasks.verify_store = policy;
        Ok(self)
    }
    /// Attach immutable front dispatch before any factory shares this request.
    /// The explicit Lower read replaces eager completion for this request.
    pub fn with_front_lower(
        mut self: Arc<Self>,
        front: NativeFrontLower,
    ) -> Result<Arc<Self>, String> {
        for path in front.bindings.keys() {
            if !self.source.declarations.sources.contains_key(path) {
                return Err("front Lower binding has no native source declaration".into());
            }
        }
        let tasks =
            Arc::get_mut(&mut self).ok_or("native tasks were shared before front binding")?;
        if tasks.front.is_some() {
            return Err("native front Lower dispatch is already bound".into());
        }
        let mut metadata = hirsel_engine::NativeMetadataWeight::default();
        metadata.map(&front.bindings, |w, path, _| w.path(path));
        let mut registry = tasks.registry.lock().unwrap_or_else(|e| e.into_inner());
        registry.charged = registry
            .charged
            .checked_add(metadata.bytes())
            .filter(|total| *total <= tasks.allowance.bytes())
            .ok_or("native front dispatch metadata exceeds retained allowance")?;
        drop(registry);
        tasks.front = Some(front);
        Ok(self)
    }
    pub(super) fn retained_allowance(&self) -> Arc<MemoryReservation> {
        self.allowance.clone()
    }
    /// Keep copied source declarations charged when a settled native snapshot
    /// outlives this task registry. The registry allowance is a separate slice.
    pub fn retained_source_allowance(&self) -> Arc<MemoryReservation> {
        self.source
            .metadata_allowance()
            .expect("instance source metadata admitted at construction")
    }
    fn producer(&self) -> DemandInstances {
        DemandInstances::with_validator(
            &self.source.provider(),
            self.store.clone(),
            self.options.target.clone(),
            self.policy,
            self.native.clone(),
        )
        .with_store_verification(self.verify_store)
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
                        source: OnceLock::new(),
                        output: OnceLock::new(),
                    }),
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        for (id, input, charge, entry) in entries {
            if let Some(previous) = registry.entries.get(&id) {
                if previous.key != entry.key || previous.request != entry.request {
                    return Err(format!(
                        "inconsistent native request registration for {}: previous={:?}, requested={:?}",
                        entry.key.instance.definition(),
                        previous.request,
                        entry.request,
                    ));
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
            &output.seal_demand,
            &output.callees,
            &output.selection_reads,
            &output.constant_reads,
            &output.templates,
            &output.operation_freedom,
            &output.emission,
            &output.discharge,
        ))
        .map_err(|e| e.to_string())?;
        let bytes = ordinary
            .len()
            .saturating_add(hirsel_codec::to_text(&output.adapters).len())
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
    /// Reopen a bounded native diagnostic. This conveys no product or verifier
    /// authority and never reruns source, instance, C or Ready work.
    pub fn failure_message(&self, reference: Reference) -> Option<String> {
        if reference.domain != FAILURE || reference.len > self.weight.retained {
            return None;
        }
        let bytes = self.store.get_content(reference)?;
        if FAILURE.reference(&bytes) != Some(reference) {
            return None;
        }
        std::str::from_utf8(&bytes).ok().map(str::to_owned)
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
    /// Current independent source V rows for all actual completed source
    /// selections, assembled without source or candidate native body reads.
    fn current_source(&self) -> Result<super::source_inputs::FrozenNativeSource, String> {
        let entries = self.entries();
        let source = if let Some(front) = &self.front {
            let mut selected = BTreeMap::new();
            for entry in &entries {
                if let Some(path) = entry.request.source_definition() {
                    let (product, original) = entry
                        .source
                        .get()
                        .ok_or("completed source instance has no settled Lower selection")?;
                    if let Some((previous, _)) = selected.get(path) {
                        if previous != product {
                            return Err("source instances selected different Lower outputs".into());
                        }
                    } else {
                        selected.insert(path.clone(), (*product, original.clone()));
                    }
                }
            }
            self.source.provider_from_products(
                &front.context,
                selected.into_values().map(|(_, original)| original),
                self.store.clone(),
            )?
        } else {
            self.source.provider()
        };
        Ok(source)
    }
    pub fn source_operations(
        &self,
        settled: ProductRef,
    ) -> Result<hirsel_layout::operation::OperationRegistry, String> {
        if self.completed.get() != Some(&settled) {
            return Err("source operation inputs require settled native discovery".into());
        }
        let entries = self.entries();
        let mut source = self.current_source()?;
        let mut registry = self.source.declarations.program.operation_registry();
        self.producer().bind_planned_adapters(
            &source,
            entries.iter().map(|entry| &entry.request),
            &mut registry,
        )?;
        for entry in entries {
            let Some(path) = entry.request.source_definition() else {
                continue;
            };
            let selected = hirsel_lir::ir::Symbol {
                kind: hirsel_lir::ir::SymbolKind::Function,
                definition: entry.key.instance.definition().clone(),
                instance: Some(entry.key.instance.digest()),
            };
            let (evidence, _) = source.source_invocations(
                path,
                &selected,
                hirsel_tir::BodyId(entry.key.instance.body()),
            )?;
            if let Some(previous) = registry.invocations.insert(
                evidence.body.definition.clone(),
                evidence.invocations.clone(),
            ) && previous != evidence.invocations
            {
                return Err("source invocation inventories differ within the request".into());
            }
            registry.invocation_bodies.push(evidence.body);
        }
        registry
            .invocation_bodies
            .sort_by(|a, b| (&a.definition, a.instance).cmp(&(&b.definition, b.instance)));
        Ok(registry)
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
        products.recheck_receipts(&self.current_source()?)?;
        products.bind_original_roots(&self.roots)?;
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
    /// Actual independently compared check removals for this complete native
    /// discovery. Ordinary per-instance facts cannot certify a partial graph.
    pub fn discharged(
        &self,
        settled: ProductRef,
    ) -> Result<hirsel_lir::discharge::Discharge, String> {
        let completed = self.completed_instances(settled)?;
        super::discharge::complete(
            &self.options.discharge,
            completed
                .iter()
                .map(|(key, output)| (key.instance.definition(), &output.discharge)),
        )
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
        if let Some(result) = self.terminal {
            return finish(result);
        }
        let mut receipt_products = Vec::new();
        if let Some(front) = &self.tasks.front
            && let Some(owner) = self.entry.request.source_definition()
            && (self.id.kind == INSTANCE_INPUT_KIND || self.entry.request.has_receipt_roots())
        {
            let paths = if self.entry.request.has_receipt_roots() {
                self.tasks
                    .source
                    .declarations
                    .context
                    .sources
                    .keys()
                    .collect::<Vec<_>>()
            } else {
                vec![owner]
            };
            let mut waiting = Vec::new();
            for path in paths {
                let Some(query) = front.bindings.get(path).copied() else {
                    let error = self
                        .tasks
                        .failure("native source has no front Lower dependency");
                    self.terminal = Some(Err(error));
                    return finish(Err(error));
                };
                let selected = match cx.read(query) {
                    Poll::Pending => {
                        waiting.push(query);
                        continue;
                    }
                    Poll::Ready(Err(error)) => {
                        self.terminal = Some(Err(error));
                        return finish(Err(error));
                    }
                    Poll::Ready(Ok(product)) if product.address == query.address => product,
                    _ => {
                        let error = self
                            .tasks
                            .failure("front Lower returned another query output");
                        self.terminal = Some(Err(error));
                        return finish(Err(error));
                    }
                };
                let result = front
                    .products
                    .lower_product(path, selected)
                    .and_then(|product| {
                        if &product.definition != path {
                            return Err("front Lower names another source definition".into());
                        }
                        if path == owner {
                            if let Some((previous, _)) = self.entry.source.get() {
                                if *previous != selected {
                                    return Err("front Lower changed within a native input".into());
                                }
                            } else {
                                self.entry.source.set((selected, product.clone())).map_err(
                                    |_| "front Lower completion raced within one task".to_owned(),
                                )?;
                            }
                        }
                        receipt_products.push(product);
                        Ok(())
                    });
                if let Err(message) = result {
                    let error = self.tasks.failure(&message);
                    self.terminal = Some(Err(error));
                    return finish(Err(error));
                }
            }
            if !waiting.is_empty() {
                return QueryPoll::Waiting(waiting);
            }
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
            let mut source = if let Some(front) = &self.tasks.front
                && self.entry.request.source_definition().is_some()
            {
                let (_, product) = self
                    .entry
                    .source
                    .get()
                    .ok_or("native input has no settled front Lower")?;
                let products = if self.entry.request.has_receipt_roots() {
                    receipt_products
                } else {
                    vec![product.clone()]
                };
                self.tasks.source.provider_from_products(
                    &front.context,
                    products,
                    self.tasks.store.clone(),
                )?
            } else {
                self.tasks.source.provider()
            };
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
    fn continuation_bytes(&self) -> u64 {
        // Completed products remain owned by the registry/snapshot and its
        // explicit reservation. This charge covers only the task frame.
        self.tasks.weight.retained
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
            // A source definition's licensed sites may span several bodies.
            // Only the complete fixed point can prove none were missed.
            let entries = self.tasks.entries();
            let facts = entries
                .iter()
                .map(|entry| {
                    let output = entry.output.get().ok_or("discovery output is unfinished")?;
                    Ok((entry.key.instance.definition(), &output.discharge))
                })
                .collect::<Result<Vec<_>, String>>()?;
            super::discharge::complete(&self.tasks.options.discharge, facts)?;
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
