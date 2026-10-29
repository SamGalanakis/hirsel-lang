//! One runtime request from cold instance discovery through actual native Link.
//! Dispatch is installed before the request; dependent task metadata is attached
//! only after its actual arena/instance reads settle. Creation never runs work.
use super::{
    arena_tasks::ArenaTasks,
    instance_tasks::InstanceTasks,
    link_tasks::{
        LINK_INPUT_KIND, LINK_MANIFEST_KIND, LinkInputFactory, LinkTasks, NativeLinkExecutor,
        NativeLinkPlan,
    },
    products::{EmissionOptions, NativeSnapshot},
    root_tasks::{NativeRootFactory, NativeRootTasks},
    tasks::{CodegenTasks, NativeTaskFactory, NativeTasks},
    *,
};
use crate::native::object_queries::{
    CODEGEN_KIND, CodegenKey, LINK_KIND, LinkKey, ObjectTaskFactory,
};
use crate::native::root_source::NativeLinkSource;
use hirsel_engine::runtime::*;
use hirsel_engine::store::{Action, Address, Domain};
use std::{collections::BTreeMap, sync::OnceLock, task::Poll};

pub const COMPILATION_KIND: u32 = 0x4e00_0023;
/// Frozen platform/byte/codegen identities. The discovered instance closure is
/// applied during the compilation poll; no mutable engine or callback builds
/// hidden native dependencies here. Executor is invoked solely by actual Link.
pub struct CompilationBackend {
    pub backend: String,
    pub backend_version: ContentHash,
    pub target_features: ContentHash,
    pub profile_options: ContentHash,
    pub runtime_abi: ContentHash,
    pub linker_options: ContentHash,
    pub runtime_archives: Vec<Reference>,
    pub foreign_inputs: Vec<Reference>,
    pub executor: Arc<dyn NativeLinkExecutor>,
}
struct Dispatch {
    discovery: ProductRef,
    arena: ProductRef,
    native: Arc<NativeTasks>,
    roots: Arc<NativeRootTasks>,
    link: Arc<LinkTasks>,
    factories: BTreeMap<u32, Arc<dyn QueryFactory>>,
    links: Vec<QueryId>,
}
/// Immutable inputs, then compact dispatch and completion metadata. C owns its
/// own private acceptance caches. No Definition or Value collection is retained.
pub struct CompilationTasks {
    instances: Arc<InstanceTasks>,
    arena: Arc<ArenaTasks>,
    source: Arc<NativeLinkSource>,
    root: NativeRootKey,
    emission: EmissionOptions,
    backends: Vec<CompilationBackend>,
    query: QueryId,
    verify_store: hirsel_engine::store::VerifyStore,
    emit_only: bool,
    weight: MemoryWeight,
    allowance: Arc<MemoryReservation>,
    dispatch: OnceLock<Dispatch>,
    completed: OnceLock<(ProductRef, Vec<ProductRef>)>,
    fallback: Reference,
}
impl CompilationTasks {
    pub fn new(
        instances: Arc<InstanceTasks>,
        arena: Arc<ArenaTasks>,
        source: Arc<NativeLinkSource>,
        root: NativeRootKey,
        emission: EmissionOptions,
        backends: Vec<CompilationBackend>,
        weight: MemoryWeight,
        allowance: MemoryReservation,
    ) -> Result<Arc<Self>, String> {
        Self::new_with_store_verification(
            instances,
            arena,
            source,
            root,
            emission,
            backends,
            weight,
            allowance,
            hirsel_engine::store::VerifyStore::Off,
        )
    }
    /// Explicit object/Link cache verification, captured before dispatch.
    /// Recompute still requires the same settled native/C dependency scope.
    pub fn new_with_store_verification(
        instances: Arc<InstanceTasks>,
        arena: Arc<ArenaTasks>,
        source: Arc<NativeLinkSource>,
        root: NativeRootKey,
        emission: EmissionOptions,
        backends: Vec<CompilationBackend>,
        weight: MemoryWeight,
        mut allowance: MemoryReservation,
        verify_store: hirsel_engine::store::VerifyStore,
    ) -> Result<Arc<Self>, String> {
        if root.target_abi != instances.options().target.digest()
            || root.policy != instances.policy()
            || backends.is_empty()
            || backends
                .iter()
                .any(|backend| !matches!(backend.backend.as_str(), "llvm" | "cranelift"))
        {
            return Err("compilation inputs differ from native request snapshot".into());
        }
        source.retain_metadata(&mut allowance)?;
        let roots = root.query_id()?;
        let selection = serde_json::to_vec(
            &backends
                .iter()
                .map(|backend| {
                    (
                        &backend.backend,
                        backend.backend_version,
                        backend.target_features,
                        backend.profile_options,
                        backend.runtime_abi,
                        backend.linker_options,
                        backend.executor.codegen_partition(),
                    )
                })
                .collect::<Vec<_>>(),
        )
        .map_err(|e| e.to_string())?;
        let query = QueryId {
            kind: COMPILATION_KIND,
            address: Address(ContentHash::of_parts(
                "hirsel.native.compilation-address.v1",
                [
                    roots.address.0.0.as_slice(),
                    selection.as_slice(),
                    &[u8::from(
                        verify_store == hirsel_engine::store::VerifyStore::Recompute,
                    )],
                ],
            )),
        };
        if weight.retained < (std::mem::size_of::<CompilationTask>() + backends.len() * 256) as u64
        {
            return Err("compilation continuation exceeds retained allowance".into());
        }
        let fallback = instances.failure("native compilation resource or protocol refusal");
        Ok(Arc::new(Self {
            instances,
            arena,
            source,
            root,
            emission,
            backends,
            query,
            verify_store,
            emit_only: false,
            weight,
            allowance: Arc::new(allowance),
            dispatch: OnceLock::new(),
            completed: OnceLock::new(),
            fallback,
        }))
    }
    /// Select object emission before installing any factory. This uses the
    /// same discovery, arena, independent C and Ready/Codegen dependencies;
    /// no Link task is demanded and no executable is manufactured.
    pub fn with_codegen_output(mut self: Arc<Self>) -> Result<Arc<Self>, String> {
        let this =
            Arc::get_mut(&mut self).ok_or("Codegen output requires an unshared compilation")?;
        if this.dispatch.get().is_some() || this.completed.get().is_some() {
            return Err("Codegen output must be selected before dispatch".into());
        }
        if !this.emit_only {
            this.query.address = Address(ContentHash::of_parts(
                "hirsel.native.compilation-codegen-address.v1",
                [this.query.address.0.0.as_slice()],
            ));
            this.emit_only = true;
        }
        Ok(self)
    }
    /// The diagnostic produced by an actual native dependency, including a
    /// failure before compilation completes. Reopening it executes no query.
    pub fn failure(&self, reference: Reference) -> Option<String> {
        self.instances.failure_message(reference)
    }
    pub fn query_id(&self) -> QueryId {
        self.query
    }
    /// Installable dispatch for this one immutable compilation request. Creating
    /// factories only describes work; discovery, arena solving, native admission,
    /// emission and linking still run solely through their dependency reads.
    pub fn factories(self: &Arc<Self>) -> Vec<(u32, Arc<dyn QueryFactory>)> {
        let instances: Arc<dyn QueryFactory> = Arc::new(
            super::instance_tasks::InstanceTaskFactory::new(self.instances.clone()),
        );
        let arena: Arc<dyn QueryFactory> = Arc::new(super::arena_tasks::ArenaTaskFactory::new(
            self.arena.clone(),
        ));
        let compilation: Arc<dyn QueryFactory> = Arc::new(CompilationFactory::new(self.clone()));
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
            COMPILATION_KIND,
            super::seal_demand::SEAL_DEMAND_KIND,
            READY_KIND,
            ROOT_KIND,
            CODEGEN_KIND,
            LINK_KIND,
            LINK_INPUT_KIND,
            LINK_MANIFEST_KIND,
            super::execution::EXECUTION_KIND,
            super::execution::EXECUTION_OBJECTS_KIND,
        ] {
            factories.push((kind, compilation.clone()));
        }
        factories
    }
    fn initialize(&self, discovery: ProductRef, arena: ProductRef) -> Result<&Dispatch, String> {
        if let Some(dispatch) = self.dispatch.get() {
            return if dispatch.discovery == discovery && dispatch.arena == arena {
                Ok(dispatch)
            } else {
                Err("compilation dependency changed within immutable snapshot".into())
            };
        }
        let completed = self.instances.completed_instances(discovery)?;
        hirsel_intern::memory::checkpoint("global Value complete");
        let guard_bytes = if completed
            .iter()
            .any(|(_, output)| output.facts.seal_demand.has_seal())
        {
            super::seal_demand::Plan::admission_bytes(
                completed
                    .iter()
                    .map(|(_, output)| &output.facts.seal_demand),
            )
        } else {
            0
        };
        let needs_execution = completed
            .iter()
            .any(|(_, output)| output.facts.templates.execution);
        let owners = completed
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let declarations = &self.instances.source_inputs().declarations;
        let mut operations = self.instances.source_operations(discovery)?;
        let operations_bytes = super::ownership_compilation::metadata::operations(&mut operations);
        let manifest = declarations.manifest()?;
        let mut snapshot = NativeSnapshot::new(
            self.instances.selected_store().clone(),
            self.instances.products(discovery)?,
            self.arena.graph(arena)?,
            self.instances.options().clone(),
            self.emission.clone(),
            manifest,
            Arc::new(operations),
        )?
        .with_store_verification(self.verify_store)?;
        // The returned NativeTasks, snapshot and linker views can outlive this
        // controller. Share each existing charge with their actual data owner;
        // never reserve a second copy or retain a controller back-reference.
        let retained_allowances = vec![
            self.allowance.clone(),
            self.instances.retained_allowance(),
            self.instances.retained_source_allowance(),
            self.source
                .metadata_allowance()
                .ok_or("native link source metadata is not admitted")?,
            self.arena.retained_allowance(),
        ];
        Arc::get_mut(&mut snapshot)
            .ok_or("native snapshot was shared before admission completed")?
            .retained_allowances = retained_allowances;
        Arc::get_mut(&mut snapshot).unwrap().guard_admitted_bytes = guard_bytes;
        let snapshot_bytes = Arc::get_mut(&mut snapshot)
            .unwrap()
            .retained_dispatch_bytes();
        let mut native = NativeTasks::new(snapshot, self.weight)?;
        let native_bytes = Arc::get_mut(&mut native).unwrap().retained_dispatch_bytes();
        let roots = NativeRootTasks::new(
            native.clone(),
            self.source.clone(),
            self.root.clone(),
            self.weight,
        )?;
        let mut codegen = Vec::new();
        let mut whole_codegen = Vec::new();
        let mut plans = Vec::new();
        let mut links = Vec::new();
        for backend in &self.backends {
            let fragments = owners
                .iter()
                .map(|owner| CodegenKey {
                    owner: owner.clone(),
                    backend: backend.backend.clone(),
                    backend_version: backend.backend_version,
                    target_features: backend.target_features,
                    profile_options: backend.profile_options,
                    runtime_abi: backend.runtime_abi,
                })
                .collect::<Vec<_>>();
            match backend.executor.codegen_partition() {
                super::link_tasks::CodegenPartition::PerInstance => {
                    if self.emit_only {
                        links.extend(fragments.iter().map(CodegenKey::query_id));
                    }
                    codegen.extend(fragments.iter().cloned());
                }
                partition => {
                    for key in partition.modules(&fragments)? {
                        if self.emit_only {
                            links.push(key.query_id());
                        }
                        whole_codegen.push(key);
                    }
                }
            }
            if self.emit_only {
                continue;
            }
            let key = LinkKey {
                roots: owners.clone().into(),
                target: self.root.target_abi,
                linker_options: backend.linker_options,
            };
            links.push(key.query_id());
            plans.push(NativeLinkPlan {
                key,
                fragments,
                runtime_archives: backend.runtime_archives.clone(),
                foreign_inputs: backend.foreign_inputs.clone(),
                executor: backend.executor.clone(),
                weight: self.weight,
            });
        }
        let execution_keys = self
            .backends
            .iter()
            .filter(|_| needs_execution)
            .map(|backend| CodegenKey {
                owner: owners[0].clone(),
                backend: backend.backend.clone(),
                backend_version: backend.backend_version,
                target_features: backend.target_features,
                profile_options: backend.profile_options,
                runtime_abi: backend.runtime_abi,
            })
            .collect::<Vec<_>>();
        let mut execution =
            super::execution::ExecutionTasks::new(roots.clone(), &execution_keys, self.weight);
        let execution_bytes = Arc::get_mut(&mut execution)
            .unwrap()
            .retained_dispatch_bytes();
        let mut objects =
            CodegenTasks::with_whole_modules(native.clone(), codegen, whole_codegen, self.weight)?;
        Arc::get_mut(&mut objects).unwrap().execution =
            needs_execution.then(|| execution.query_id());
        let object_bytes = Arc::get_mut(&mut objects)
            .unwrap()
            .retained_dispatch_bytes();
        let mut link = LinkTasks::new(roots.clone(), plans, self.weight)?;
        Arc::get_mut(&mut link).unwrap().execution = needs_execution.then(|| execution.clone());
        let link_bytes = Arc::get_mut(&mut link).unwrap().retained_dispatch_bytes();
        let mut copied = hirsel_engine::NativeMetadataWeight::default();
        // Fixed controller/factory envelopes plus every owned variable buffer.
        copied.add(16384);
        copied.vec(&self.root.roots, |w, path| w.path(path));
        super::ownership_compilation::metadata::emission(&mut copied, &self.emission);
        copied.vec(&self.backends, |w, backend| {
            w.add(backend.backend.capacity());
            w.flat_vec(&backend.runtime_archives);
            w.flat_vec(&backend.foreign_inputs);
        });
        copied.flat_vec(&links);
        copied.add(
            links
                .len()
                .saturating_mul(2)
                .max(4)
                .saturating_mul(size_of::<ProductRef>()),
        );
        let bytes = guard_bytes
            .saturating_add(operations_bytes)
            .saturating_add(snapshot_bytes)
            .saturating_add(native_bytes)
            .saturating_add(roots.retained_dispatch_bytes())
            .saturating_add(execution_bytes)
            .saturating_add(object_bytes)
            .saturating_add(link_bytes)
            .saturating_add(copied.bytes());
        if bytes > self.allowance.bytes() {
            return Err("native compilation dispatch allowance exhausted".into());
        }
        let leaf: Arc<dyn QueryFactory> = Arc::new(LinkInputFactory::new(link.clone()));
        let mut factories: BTreeMap<u32, Arc<dyn QueryFactory>> = BTreeMap::from([
            (
                READY_KIND,
                Arc::new(NativeTaskFactory::new(native.clone())) as Arc<dyn QueryFactory>,
            ),
            (
                ROOT_KIND,
                Arc::new(NativeRootFactory::new(roots.clone())) as Arc<dyn QueryFactory>,
            ),
            (
                CODEGEN_KIND,
                Arc::new(ObjectTaskFactory::new(objects)) as Arc<dyn QueryFactory>,
            ),
            (
                LINK_KIND,
                Arc::new(ObjectTaskFactory::new(link.clone())) as Arc<dyn QueryFactory>,
            ),
            (LINK_INPUT_KIND, leaf.clone()),
            (LINK_MANIFEST_KIND, leaf),
        ]);
        let execution_factory: Arc<dyn QueryFactory> =
            Arc::new(super::execution::Factory(execution));
        factories.insert(super::execution::EXECUTION_KIND, execution_factory.clone());
        factories.insert(super::execution::EXECUTION_OBJECTS_KIND, execution_factory);
        if let Some((key, factory)) = native.guard_factory() {
            factories.insert(key.kind, factory);
        }
        self.dispatch
            .set(Dispatch {
                discovery,
                arena,
                native,
                roots,
                link,
                factories,
                links,
            })
            .map_err(|_| "duplicate native compilation dispatch construction")?;
        Ok(self.dispatch.get().unwrap())
    }
    fn selected(&self, compilation: ProductRef) -> Result<&Dispatch, String> {
        let (selected, _) = self
            .completed
            .get()
            .ok_or("native compilation has not completed")?;
        if *selected != compilation {
            return Err("native compilation differs from selected output".into());
        }
        self.dispatch
            .get()
            .ok_or("native compilation dispatch is unavailable".into())
    }
    /// Exact selected outputs, in backend order: Link for executables and
    /// Codegen fragments for emit-only requests.
    pub fn outputs(&self, compilation: ProductRef) -> Result<&[ProductRef], String> {
        self.selected(compilation)?;
        Ok(&self.completed.get().unwrap().1)
    }
    /// Number of independently admitted instances emitted by this completion.
    pub fn emitted_instances(&self, compilation: ProductRef) -> Result<usize, String> {
        let dispatch = self.selected(compilation)?;
        if !self.emit_only {
            return Err("compilation selected an executable, not object emission".into());
        }
        Ok(self
            .instances
            .completed_instances(dispatch.discovery)?
            .len())
    }
    pub fn native(&self, compilation: ProductRef) -> Result<Arc<NativeTasks>, String> {
        Ok(self.selected(compilation)?.native.clone())
    }
    pub fn roots(&self, compilation: ProductRef) -> Result<Arc<NativeRootTasks>, String> {
        Ok(self.selected(compilation)?.roots.clone())
    }
    /// Actual independently validated removals from the exact discovery read
    /// by this completed compilation. The requested plan supplies no report.
    pub fn discharged(
        &self,
        compilation: ProductRef,
    ) -> Result<hirsel_lir::discharge::Discharge, String> {
        self.instances
            .discharged(self.selected(compilation)?.discovery)
    }
    /// Body-free execution metadata of this exact completed compilation's
    /// settled Link scope. No dependency query or current-association lookup
    /// supplies a missing read, and Link hits expose the same admitted metadata.
    pub fn observation(
        &self,
        compilation: ProductRef,
    ) -> Result<crate::requests::NativeObservation, String> {
        self.selected(compilation)?.link.observation()
    }

    fn publish(&self, outputs: Vec<ProductRef>) -> Result<ProductRef, String> {
        if !self.emit_only {
            hirsel_intern::memory::checkpoint("link complete");
        }
        let bytes = serde_json::to_vec(
            &outputs
                .iter()
                .map(|product| (product.address.0, product.record))
                .collect::<Vec<_>>(),
        )
        .map_err(|e| e.to_string())?;
        let domain = Domain {
            kind: COMPILATION_KIND,
            schema: 1,
        };
        let store = self.instances.selected_store();
        let record = store
            .put_content(domain, &bytes)
            .ok_or("native compilation descriptor transport failed")?;
        if domain.reference(&bytes) != Some(record) {
            return Err("native compilation content identity differs".into());
        }
        let action = Action(ContentHash::of_parts(
            "hirsel.native.compilation-action.v1",
            [
                self.query.address.0.0.as_slice(),
                record.output.0.0.as_slice(),
            ],
        ));
        store.associate_product(self.query.address, action, record);
        let product = ProductRef {
            address: self.query.address,
            action,
            record,
        };
        if let Err(other) = self.completed.set((product, outputs))
            && self.completed.get().unwrap().0 != other.0
        {
            return Err("native compilation completion changed".into());
        }
        Ok(product)
    }
}
pub struct CompilationFactory {
    tasks: Arc<CompilationTasks>,
}
impl CompilationFactory {
    pub fn new(tasks: Arc<CompilationTasks>) -> Self {
        Self { tasks }
    }
}
impl QueryFactory for CompilationFactory {
    fn create(&self, key: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if key == self.tasks.query {
            return Ok(Box::new(CompilationTask {
                tasks: self.tasks.clone(),
                terminal: None,
            }));
        }
        self.tasks
            .dispatch
            .get()
            .and_then(|dispatch| dispatch.factories.get(&key.kind))
            .ok_or(self.tasks.fallback)?
            .create(key)
    }
}
struct CompilationTask {
    tasks: Arc<CompilationTasks>,
    terminal: Option<Result<ProductRef, Reference>>,
}
fn finish(result: Result<ProductRef, Reference>) -> QueryPoll {
    match result {
        Ok(product) => QueryPoll::Complete(product),
        Err(error) => QueryPoll::Failed(error),
    }
}
impl QueryTask for CompilationTask {
    fn id(&self) -> QueryId {
        self.tasks.query
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
impl CompilationTask {
    fn advance(
        &self,
        cx: &mut dyn QueryContext,
    ) -> Result<Result<ProductRef, Vec<QueryId>>, Reference> {
        let tasks = &self.tasks;
        let mut waiting = Vec::new();
        let mut selected = Vec::new();
        for key in [tasks.instances.discovery_id(), tasks.arena.query_id()] {
            match cx.read(key) {
                Poll::Pending => waiting.push(key),
                Poll::Ready(Err(error)) => return Err(error),
                Poll::Ready(Ok(product)) => selected.push(product),
            }
        }
        if !waiting.is_empty() {
            return Ok(Err(waiting));
        }
        let dispatch = tasks
            .initialize(selected[0], selected[1])
            .map_err(|e| tasks.instances.failure(&e))?;
        let mut outputs = Vec::new();
        for key in &dispatch.links {
            match cx.read(*key) {
                Poll::Pending => waiting.push(*key),
                Poll::Ready(Err(error)) => return Err(error),
                Poll::Ready(Ok(product)) if product.address == key.address => outputs.push(product),
                _ => {
                    return Err(tasks
                        .instances
                        .failure("native compilation read another selected output"));
                }
            }
        }
        if !waiting.is_empty() {
            return Ok(Err(waiting));
        }
        tasks
            .publish(outputs)
            .map(Ok)
            .map_err(|e| tasks.instances.failure(&e))
    }
}
