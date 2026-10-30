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
    callables: OnceLock<(ProductRef, super::demand::callable::Scope)>,
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
        Self::with_library_roots(source, store, options, policy, weight, allowance, &[])
    }
    pub(crate) fn with_library_roots(
        source: Arc<NativeInstanceInputs>,
        store: Arc<dyn ResultStore>,
        options: crate::native::Options,
        policy: ContentHash,
        weight: MemoryWeight,
        mut allowance: MemoryReservation,
        library: &[hirsel_rc::library::Request],
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
        let roots = initial.roots_with_library(&provider, library)?;
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
        let root_bytes = roots.len().saturating_mul(size_of::<ContentHash>());
        if weight.retained < root_bytes as u64 + std::mem::size_of::<DiscoveryTask>() as u64
            || weight.retained < std::mem::size_of::<InstanceTask>() as u64
            || weight.retained < std::mem::size_of::<DiscoveryTask>() as u64
        {
            return Err("native task frame exceeds retained allowance".into());
        }
        let mut metadata = hirsel_engine::NativeMetadataWeight::default();
        metadata.add(size_of::<Self>() + 2 * size_of::<usize>());
        metadata.add(size_of::<MemoryReservation>() + 2 * size_of::<usize>());
        metadata.vec(&roots, |w, key| {
            w.path(key.instance.definition());
            w.add(key.instance.retained_encoding_bytes());
        });
        if metadata.bytes() > allowance.bytes() {
            return Err("native task metadata exceeds retained allowance".into());
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
                charged: metadata.bytes(),
            }),
            completed: OnceLock::new(),
            callables: OnceLock::new(),
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
        .with_retained_owner(self.allowance.clone())
    }
    fn register(&self, requests: Vec<(InstanceAddress, Request)>) -> Result<(), String> {
        // Encoding/validation runs outside the index lock. admit() has already
        // checked recipe domains and presence through the sole demand core.
        let entries = requests
            .into_iter()
            .map(|(key, request)| {
                let mut weight = hirsel_engine::NativeMetadataWeight::default();
                weight.add(size_of::<Registered>() + 2 * size_of::<usize>());
                weight.path(key.instance.definition());
                weight.add(key.instance.retained_encoding_bytes());
                let charge = weight
                    .bytes()
                    .saturating_add(request.retained_metadata_bytes());
                (
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
                )
            })
            .collect::<Vec<_>>();
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        let index_bytes = |len| {
            let mut weight = hirsel_engine::NativeMetadataWeight::default();
            weight.tree::<ContentHash, Arc<Registered>>(len);
            weight.bytes().saturating_mul(2)
        };
        for (id, input, mut charge, entry) in entries {
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
            charge = charge.saturating_add(index_bytes(registry.entries.len() + 1).saturating_sub(
                if registry.entries.is_empty() {
                    0
                } else {
                    index_bytes(registry.entries.len())
                },
            ));
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
        let charge = output.retained_metadata_bytes();
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        if charge > self.allowance.bytes().saturating_sub(registry.charged) {
            let declarations = &self.source.declarations;
            let mut constants = hirsel_engine::NativeMetadataWeight::default();
            for value in declarations.program.constants.values() {
                constants.materialized(value);
            }
            let mut source_constants = hirsel_engine::NativeMetadataWeight::default();
            for source in declarations.sources.values() {
                if let Some(value) = &source.constant {
                    source_constants.materialized(value);
                }
                for (_, value) in &source.materialized {
                    source_constants.materialized(value);
                }
            }
            let requests = registry
                .entries
                .values()
                .map(|entry| entry.request.retained_metadata_bytes())
                .sum::<u64>();
            let descriptors = registry
                .entries
                .values()
                .filter_map(|entry| entry.output.get())
                .map(|output| output.retained_metadata_bytes())
                .sum::<u64>();
            return Err(format!(
                "native completed descriptor allowance exhausted for {}: requested {charge} bytes, retained {}, allowance {}, registered {}, ready {}, seal {}, freedom {}, discharge {}, source metadata {}, program constants {}, source constants {}, front bindings {}, request payloads {requests}, completed descriptors {descriptors}",
                output.facts.body.seal_demand.owner().0,
                registry.charged,
                self.allowance.bytes(),
                registry.entries.len(),
                output.facts.source.ready_source.retained_metadata_bytes(),
                output.facts.body.seal_demand.retained_bytes(),
                output.facts.body.operation_freedom.retained_bytes(),
                output.facts.source.discharge.retained_metadata_bytes(),
                self.source.retained_metadata_bytes(),
                constants.bytes(),
                source_constants.bytes(),
                self.front.as_ref().map_or(0, |front| front.bindings.len()),
            ));
        }
        registry.charged += charge;
        Ok(())
    }
    fn put(&self, key: QueryId, bytes: Vec<u8>) -> Result<ProductRef, String> {
        let domain = Domain {
            kind: key.kind,
            schema: 1,
        };
        let record = self
            .store
            .put_content(domain, &bytes)
            .ok_or("native task output transport failed")?;
        if domain.reference(&bytes) != Some(record) {
            return Err("native task output content identity differs".into());
        }
        let action = Action(ContentHash::of_parts(
            "hirsel.native.task-output-action.v1",
            [key.address.0.0.as_slice(), record.output.0.0.as_slice()],
        ));
        self.store.associate_product(key.address, action, record);
        Ok(ProductRef::import_archive(
            key.address,
            action,
            self.store
                .admit_product_archive(record, bytes.into())
                .ok_or("native product archive is unavailable or exceeds admission")?,
        ))
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
                        selected.insert(path.clone(), (product.clone(), original.clone()));
                    }
                }
            }
            self.source.provider_from_products(
                &front.context,
                selected.into_values().map(|(_, original)| original),
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
        if let Some((_, scope)) = self.callables.get() {
            return Ok(scope.operations.clone());
        }
        self.current_operations()
    }
    fn current_operations(&self) -> Result<hirsel_layout::operation::OperationRegistry, String> {
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
        let mut products = self.original_products()?;
        if let Some((_, scope)) = self.callables.get() {
            products.use_callable_scope(scope);
        }
        Ok(products)
    }
    fn original_products(&self) -> Result<DemandInstances, String> {
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
        if let Some((_, scope)) = self.callables.get() {
            return Ok(scope.outputs.clone());
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
                .map(|(key, output)| (key.instance.definition(), &output.facts.source.discharge)),
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
        if id == self.tasks.callable_id() || id.kind == super::instance::CALLABLE_VALUE_KIND {
            return Ok(Box::new(CallableTask {
                tasks: self.tasks.clone(),
                id,
                terminal: None,
            }));
        }
        if id == self.tasks.discovery {
            return Ok(Box::new(DiscoveryTask {
                tasks: self.tasks.clone(),
                frontier: self
                    .tasks
                    .roots
                    .iter()
                    .map(|key| key.address(INSTANCE_KIND).0)
                    .collect(),
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
        if let Some(result) = self.terminal.clone() {
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
                    .lower_product(path, selected.clone())
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
                                let mut weight = hirsel_engine::NativeMetadataWeight::default();
                                weight.path(&product.definition);
                                let mut registry = self
                                    .tasks
                                    .registry
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                registry.charged = registry
                                    .charged
                                    .checked_add(weight.bytes())
                                    .filter(|total| *total <= self.tasks.allowance.bytes())
                                    .ok_or("native source binding exceeds retained allowance")?;
                                drop(registry);
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
                self.tasks
                    .source
                    .provider_from_products(&front.context, products)?
            } else {
                self.tasks.source.provider()
            };
            if self.id.kind == INSTANCE_INPUT_KIND {
                let bytes =
                    producer.producer_inputs(&source, &self.entry.key, &self.tasks.options)?;
                let product = self.tasks.put(self.id, bytes)?;
                if self.entry.input.set(product.clone()).is_err()
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
            let product = output.product.product.clone();
            if let Err(other) = self.entry.output.set(output)
                && self.entry.output.get().unwrap().product.product != other.product.product
            {
                return Err("native instance completion changed".into());
            }
            Ok(product)
        })()
        .map_err(|error: String| self.tasks.failure(&error));
        self.terminal = Some(result.clone());
        finish(result)
    }
}
struct DiscoveryTask {
    tasks: Arc<InstanceTasks>,
    frontier: Vec<ContentHash>,
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
        if let Some(result) = self.terminal.clone() {
            return finish(result);
        }
        let result = self.advance(cx);
        match result {
            Ok(Some(product)) => {
                self.terminal = Some(Ok(product.clone()));
                finish(Ok(product))
            }
            Ok(None) => QueryPoll::Waiting(if self.frontier.is_empty() {
                vec![self.tasks.callable_id()]
            } else {
                self.frontier
                    .iter()
                    .map(|address| QueryId {
                        kind: INSTANCE_KIND,
                        address: Address(*address),
                    })
                    .collect()
            }),
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
        while let Some(address) = frontier.pop() {
            if self.seen.contains(&address) || !read.insert(address) {
                continue;
            }
            match cx.read(QueryId {
                kind: INSTANCE_KIND,
                address: Address(address),
            }) {
                Poll::Pending => self.frontier.push(address),
                Poll::Ready(Err(error)) => return Err(error),
                Poll::Ready(Ok(product)) => {
                    let entry = self
                        .tasks
                        .registry
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .entries
                        .get(&address)
                        .cloned()
                        .ok_or_else(|| {
                            self.tasks.failure("unregistered native discovery address")
                        })?;
                    let output = entry.output.get().ok_or_else(|| {
                        self.tasks
                            .failure("native task returned no completed descriptor")
                    })?;
                    if output.product.product != product {
                        return Err(self.tasks.failure("native discovery read another output"));
                    }
                    self.seen.insert(address);
                    frontier.extend(
                        output
                            .facts
                            .body
                            .dependencies
                            .iter()
                            .map(|key| key.address(INSTANCE_KIND).0),
                    );
                }
            }
            let mut weight = hirsel_engine::NativeMetadataWeight::default();
            weight.add(size_of::<Self>());
            weight.flat_vec(&frontier);
            weight.flat_vec(&self.frontier);
            weight.tree::<ContentHash, ()>(self.seen.len());
            weight.tree::<ContentHash, ()>(read.len());
            if weight.bytes() > self.tasks.weight.retained {
                return Err(self
                    .tasks
                    .failure("native discovery frame exceeds retained allowance"));
            }
        }
        self.frontier.sort_unstable();
        self.frontier.dedup();
        if !self.frontier.is_empty() {
            return Ok(None);
        }
        let needs_callable = self.tasks.entries().iter().any(|entry| {
            entry
                .output
                .get()
                .is_some_and(|out| out.facts.body.templates.execution)
        });
        if needs_callable {
            match cx.read(self.tasks.callable_id()) {
                Poll::Pending => return Ok(None),
                Poll::Ready(Err(error)) => return Err(error),
                Poll::Ready(Ok(product))
                    if self
                        .tasks
                        .callables
                        .get()
                        .is_some_and(|(selected, _)| *selected == product) =>
                {
                    ()
                }
                _ => {
                    return Err(self
                        .tasks
                        .failure("callable graph returned another product"));
                }
            }
        }
        let result = (|| {
            // A source definition's licensed sites may span several bodies.
            // Only the complete fixed point can prove none were missed.
            let entries = self.tasks.entries();
            let facts = entries
                .iter()
                .map(|entry| {
                    let output = entry.output.get().ok_or("discovery output is unfinished")?;
                    Ok((
                        entry.key.instance.definition(),
                        &output.facts.source.discharge,
                    ))
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
                        .product
                        .clone();
                    Ok((
                        entry.key.instance.encoding().to_vec(),
                        product.address.0,
                        product
                            .archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                            .map_err(|e| e.to_string())?
                            .reference(),
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            let bytes = serde_json::to_vec(&rows).map_err(|e| e.to_string())?;
            let product = self.tasks.put(self.tasks.discovery, bytes)?;
            if self.tasks.completed.set(product.clone()).is_err()
                && self.tasks.completed.get() != Some(&product)
            {
                return Err("native discovery completion changed".into());
            }
            Ok(Some(product))
        })();
        result.map_err(|e: String| self.tasks.failure(&e))
    }
}

pub const CALLABLE_GRAPH_KIND: u32 = 0x4e00_0033;
impl InstanceTasks {
    fn callable_id(&self) -> QueryId {
        QueryId {
            kind: CALLABLE_GRAPH_KIND,
            address: Address(ContentHash::of_parts(
                "hirsel.native.callable-graph-address.v1",
                [self.discovery.address.0.0.as_slice()],
            )),
        }
    }
    fn callable_scope(&self) -> Result<ProductRef, String> {
        if let Some((product, _)) = self.callables.get() {
            return Ok(product.clone());
        }
        let mut products = self.original_products()?;
        let operations = self.current_operations()?;
        let manifest = self.source.declarations.manifest()?;
        let mut source = self.current_source()?;
        let mut scope = products.specialize_callables(
            &self.options,
            &manifest,
            operations,
            source.query_policy().reuse,
            |key, operations| {
                if source
                    .declarations()
                    .sources
                    .contains_key(key.instance.definition())
                {
                    let symbol = hirsel_lir::ir::Symbol {
                        kind: hirsel_lir::ir::SymbolKind::Function,
                        definition: key.instance.definition().clone(),
                        instance: Some(key.instance.digest()),
                    };
                    let (evidence, _) = source.source_invocations(
                        key.instance.definition(),
                        &symbol,
                        hirsel_tir::BodyId(key.instance.body()),
                    )?;
                    operations
                        .invocations
                        .insert(evidence.body.definition.clone(), evidence.invocations);
                    operations.invocation_bodies.retain(|row| {
                        row.definition != evidence.body.definition
                            || row.instance != evidence.body.instance
                    });
                    operations.invocation_bodies.push(evidence.body);
                }
                Ok(())
            },
        )?;
        {
            let entries = self.entries();
            let retained = scope.retained_bytes(
                entries.iter().map(|entry| &entry.request),
                entries
                    .iter()
                    .filter_map(|entry| entry.output.get().map(|output| &output.facts)),
            );
            let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            registry.charged = registry
                .charged
                .checked_add(retained)
                .filter(|bytes| *bytes <= self.allowance.bytes())
                .ok_or_else(|| format!(
                    "callable graph metadata exceeds retained allowance: requested {retained} bytes, retained {}, allowance {}, original instances {}, callable instances {}, inline members {}",
                    registry.charged,
                    self.allowance.bytes(),
                    entries.len(),
                    scope.outputs.len(),
                    scope.outputs.iter().map(|(_, output)| output.facts.emission.inline.len()).max().unwrap_or(0),
                ))?;
        }
        let rows = scope
            .outputs
            .iter()
            .map(|(key, output)| {
                Ok((
                    key.instance.encoding(),
                    output
                        .product
                        .product
                        .archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                        .map_err(|e| e.to_string())?
                        .reference(),
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let product = self.put(
            self.callable_id(),
            serde_json::to_vec(&rows).map_err(|e| e.to_string())?,
        )?;
        self.callables
            .set((product.clone(), scope))
            .map_err(|_| "callable graph published twice")?;
        Ok(product)
    }
}
struct CallableTask {
    tasks: Arc<InstanceTasks>,
    id: QueryId,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for CallableTask {
    fn id(&self) -> QueryId {
        self.id
    }
    fn weight(&self) -> MemoryWeight {
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
        let mut waiting = Vec::new();
        if self.id == self.tasks.callable_id() {
            for entry in self.tasks.entries() {
                let query = instance_id(&entry.key);
                match cx.read(query) {
                    Poll::Pending => waiting.push(query),
                    Poll::Ready(Err(error)) => {
                        self.terminal = Some(Err(error));
                        return finish(Err(error));
                    }
                    Poll::Ready(Ok(product))
                        if entry
                            .output
                            .get()
                            .is_some_and(|o| o.product.product == product) =>
                    {
                        ()
                    }
                    _ => {
                        let error = self.tasks.failure("callable original output differs");
                        self.terminal = Some(Err(error));
                        return finish(Err(error));
                    }
                }
            }
            if !waiting.is_empty() {
                return QueryPoll::Waiting(waiting);
            }
            let result = self
                .tasks
                .callable_scope()
                .map_err(|e| self.tasks.failure(&e));
            self.terminal = Some(result.clone());
            finish(result)
        } else {
            let query = self.tasks.callable_id();
            match cx.read(query) {
                Poll::Pending => return QueryPoll::Waiting(vec![query]),
                Poll::Ready(Err(error)) => {
                    self.terminal = Some(Err(error));
                    return finish(Err(error));
                }
                Poll::Ready(Ok(product))
                    if self
                        .tasks
                        .callables
                        .get()
                        .is_some_and(|(p, _)| *p == product) =>
                {
                    ()
                }
                _ => {
                    let error = self.tasks.failure("callable alias has another graph");
                    self.terminal = Some(Err(error));
                    return finish(Err(error));
                }
            }
            let result = self
                .tasks
                .callables
                .get()
                .unwrap()
                .1
                .outputs
                .iter()
                .find(|(_, o)| o.product.product.address == self.id.address)
                .map(|(_, o)| o.product.product.clone())
                .ok_or_else(|| self.tasks.failure("unregistered callable Value alias"));
            self.terminal = Some(result.clone());
            finish(result)
        }
    }
}

#[cfg(test)]
mod retained_tests {
    use super::*;
    use hirsel_engine::DefinitionQueries;

    #[test]
    fn discovery_frontier_keeps_addresses_within_a_small_frame() {
        hirsel_engine::with_compiler_stack(|| {
            let transport = ProcessBudget::new(64 << 20);
            let store: Arc<dyn ResultStore> = Arc::new(hirsel_engine::store::RequestStore::new(
                Arc::new(hirsel_engine::store::MemStore::default()),
                transport.reserve(64 << 20).unwrap(),
            ));
            let mut engine = hirsel_engine::QueryEngine::new(
                hirsel_engine::default_environment(),
                crate::compiler_adapters(),
            )
            .with_result_store(store.clone());
            let text = (0..48)
                .map(|n| format!("pub fn function_{n}()->u64{{1u64}}\n"))
                .collect();
            let file = engine.sources.add("frontier-bounds.hir", text).unwrap();
            engine
                .parse_file(file, &hirsel_engine::default_context())
                .unwrap();
            let mut discovery =
                hirsel_engine::NativeDiscoveryCache::new(engine.types().clone(), store.clone());
            let declarations = super::super::declarations::NativeDeclarations::collect(
                &mut engine,
                &mut discovery,
                &[],
            )
            .unwrap();
            let paths = declarations
                .context
                .sources
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            let context = engine.native_original_context(&paths).unwrap();
            let source =
                NativeInstanceInputs::from_products(declarations, context.reader([]).unwrap())
                    .unwrap();
            let budget = ProcessBudget::new(16 << 20);
            let tasks = InstanceTasks::new(
                source,
                store,
                crate::native::Options::new(hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu()),
                ContentHash::of_bytes(b"frontier bounds"),
                MemoryWeight {
                    scratch: 16 << 20,
                    retained: 16 << 10,
                    stack: 0,
                },
                budget.reserve(16 << 20).unwrap(),
            )
            .unwrap();
            assert_eq!(
                tasks
                    .roots
                    .iter()
                    .filter(|key| key.instance.definition().0.name.starts_with("function_"))
                    .count(),
                48
            );
            struct Pending(Vec<QueryId>);
            impl QueryContext for Pending {
                fn read(&mut self, query: QueryId) -> Poll<Result<ProductRef, Reference>> {
                    self.0.push(query);
                    Poll::Pending
                }
            }
            let mut task = InstanceTaskFactory::new(tasks.clone())
                .create(tasks.discovery)
                .unwrap();
            let mut pending = Pending(vec![]);
            let QueryPoll::Waiting(waiting) = task.poll(&mut pending) else {
                panic!("discovery must subscribe to all roots");
            };
            assert_eq!(waiting.len(), tasks.roots.len());
            pending.0.sort_by_key(|query| query.address.0);
            let mut waiting = waiting;
            waiting.sort_by_key(|query| query.address.0);
            assert_eq!(waiting, pending.0);
            let alias_id = QueryId {
                kind: super::super::instance::CALLABLE_VALUE_KIND,
                address: tasks.roots[0].address(super::super::instance::CALLABLE_VALUE_KIND),
            };
            for id in [tasks.callable_id(), alias_id] {
                let task = InstanceTaskFactory::new(tasks.clone()).create(id).unwrap();
                assert_eq!(task.weight().retained, size_of::<CallableTask>() as u64);
                assert_eq!(task.continuation_bytes(), size_of::<CallableTask>() as u64);
                assert_eq!(task.weight().scratch, tasks.weight.scratch);
                assert_eq!(task.weight().stack, tasks.weight.stack);
            }
        });
    }

    #[test]
    fn registry_metadata_refuses_one_less_and_releases_its_exact_partition() {
        hirsel_engine::with_compiler_stack(|| {
            let transport = ProcessBudget::new(64 << 20);
            let store: Arc<dyn ResultStore> = Arc::new(hirsel_engine::store::RequestStore::new(
                Arc::new(hirsel_engine::store::MemStore::default()),
                transport.reserve(64 << 20).unwrap(),
            ));
            let mut engine = hirsel_engine::QueryEngine::new(
                hirsel_engine::default_environment(),
                crate::compiler_adapters(),
            )
            .with_result_store(store.clone());
            let file = engine
                .sources
                .add("registry-bounds.hir", "pub fn main()->u64{1u64}".into())
                .unwrap();
            engine
                .parse_file(file, &hirsel_engine::default_context())
                .unwrap();
            let mut discovery =
                hirsel_engine::NativeDiscoveryCache::new(engine.types().clone(), store.clone());
            let declarations = super::super::declarations::NativeDeclarations::collect(
                &mut engine,
                &mut discovery,
                &[],
            )
            .unwrap();
            let paths = declarations
                .context
                .sources
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            let context = engine.native_original_context(&paths).unwrap();
            let source =
                NativeInstanceInputs::from_products(declarations, context.reader([]).unwrap())
                    .unwrap();
            let budget = ProcessBudget::new(16 << 20);
            let make = |bytes| {
                InstanceTasks::new(
                    source.clone(),
                    store.clone(),
                    crate::native::Options::new(hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu()),
                    ContentHash::of_bytes(b"registry bounds"),
                    MemoryWeight {
                        scratch: 16 << 20,
                        retained: 1 << 20,
                        stack: 0,
                    },
                    budget.reserve(bytes).unwrap(),
                )
            };
            let tasks = make(16 << 20).unwrap();
            let copied = source.retained_metadata_bytes();
            let required = tasks.registry.lock().unwrap().charged;
            assert!(required > size_of::<InstanceTasks>() as u64);
            drop(tasks);
            assert_eq!(budget.reserved(), copied);
            let refusal = make(required - 1)
                .err()
                .expect("one less cannot retain the registered roots");
            assert_eq!(refusal, "native discovery metadata allowance exhausted");
            assert_eq!(budget.reserved(), copied);
            let exact = make(required).unwrap();
            assert_eq!(exact.allowance.bytes(), required);
            assert_eq!(exact.registry.lock().unwrap().charged, required);
            assert_eq!(budget.reserved(), copied + required);
            drop(source);
            drop(exact);
            assert_eq!(budget.reserved(), 0);
        });
    }
}
