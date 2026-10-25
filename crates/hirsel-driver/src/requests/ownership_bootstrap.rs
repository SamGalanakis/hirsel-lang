//! Explicit diagnostic source root. Source V, native instance C and observed
//! Ready C settle in their own tasks. Host readiness cannot execute a producer.
use super::{native_lower, native_routing::NativeRouting, ownership_admission::Admission};
use crate::native::{
    self,
    queries::{
        self, arena_tasks::ArenaTasks, declarations::NativeDeclarations,
        instance_tasks::InstanceTasks, ownership_compilation::OwnershipCompilation,
        products::EmissionOptions, source_inputs::NativeInstanceInputs,
    },
};
use hirsel_engine::{
    FrontLowerQueries, FrontNativeProducts, FrontNativeQueries, runtime::*, store::*,
};
use hirsel_syntax::{ContentHash, DefPath};
use std::{
    sync::{Arc, Mutex, OnceLock},
    task::Poll,
};

pub(super) const KIND: u32 = 0x5200_0013;
const OUTPUT: Domain = Domain {
    kind: 0x5200_0014,
    schema: 1,
};
const FAILURE: Domain = Domain {
    kind: 0x5200_0015,
    schema: 1,
};
pub(super) const KINDS: &[u32] = &[
    queries::INSTANCE_KIND,
    queries::instance_tasks::INSTANCE_INPUT_KIND,
    queries::instance_tasks::DISCOVERY_KIND,
    queries::ARENA_CANDIDATES_KIND,
    queries::ARENA_CHOSEN_KIND,
    queries::arena_tasks::ARENA_GRAPH_KIND,
    queries::ownership_compilation::OWNERSHIP_COMPILATION_KIND,
    queries::ownership_tasks::OWNERSHIP_READY_KIND,
    queries::ownership_tasks::OWNERSHIP_ROOT_KIND,
];
pub(super) struct Source {
    pub source: Arc<FrontNativeQueries>,
    pub lower: Arc<FrontLowerQueries>,
    pub definitions: Vec<DefPath>,
    pub store: Arc<dyn ResultStore>,
}
pub(super) struct Plan {
    pub options: native::Options,
    pub policy: ContentHash,
    pub verify_store: VerifyStore,
    pub admission: Admission,
}
struct Prepared {
    source: ProductRef,
    facts: Arc<FrontNativeProducts>,
    compilation: Arc<OwnershipCompilation>,
}
pub(super) struct OwnershipBootstrap {
    source: Source,
    pending: Mutex<Option<Plan>>,
    routing: Arc<NativeRouting>,
    query: QueryId,
    weight: MemoryWeight,
    prepared: OnceLock<Prepared>,
    completed: OnceLock<(ProductRef, ProductRef)>,
    failure: Mutex<Option<(Reference, String)>>,
    errors: RuntimeErrors,
    _allowance: MemoryReservation,
    output_allowance: Mutex<MemoryReservation>,
}
impl OwnershipBootstrap {
    pub fn new(
        source: Source,
        mut plan: Plan,
        routing: Arc<NativeRouting>,
        weight: MemoryWeight,
        allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<Self>, String> {
        if plan.options.objects.is_some() || !plan.options.discharge.0.is_empty() {
            return Err("ownership root cannot use cached ordinary objects".into());
        }
        let bytes = serde_json::to_vec(&(
            source.source.query().address.0,
            plan.policy,
            plan.options.target.digest(),
            &plan.options.launch,
            format!("{:?}", plan.options.validation),
            plan.options.regions,
            plan.options.elide,
            plan.options.reuse,
            plan.options.flat_walk,
            plan.options.scoped_specialization,
        ))
        .map_err(|e| e.to_string())?;
        let mut metadata = hirsel_engine::NativeMetadataWeight::default();
        metadata.add(std::mem::size_of::<Self>() + 3 * std::mem::size_of::<usize>());
        metadata.vec(&source.definitions, |w, path| w.path(path));
        if metadata.bytes() > allowance.bytes()
            || weight.retained < std::mem::size_of::<Task>() as u64
        {
            return Err("ownership bootstrap metadata exceeds original admission".into());
        }
        let output_allowance = plan
            .admission
            .compilation
            .split(plan.admission.compilation.bytes() / 8)
            .unwrap();
        let query = QueryId {
            kind: KIND,
            address: Address(ContentHash::of_parts(
                "hirsel.host.ownership-bootstrap.v1",
                [bytes.as_slice()],
            )),
        };
        Ok(Arc::new(Self {
            source,
            pending: Mutex::new(Some(plan)),
            routing,
            query,
            weight,
            prepared: OnceLock::new(),
            completed: OnceLock::new(),
            failure: Mutex::new(None),
            errors,
            _allowance: allowance,
            output_allowance: Mutex::new(output_allowance),
        }))
    }
    pub fn query(&self) -> QueryId {
        self.query
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    pub fn reserve_output(&self, bytes: u64) -> Result<MemoryReservation, String> {
        self.output_allowance
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .split(bytes)
            .ok_or("ownership output metadata exceeds original admission".into())
    }
    pub fn outcome(
        &self,
        selected: ProductRef,
    ) -> Result<
        (
            Arc<OwnershipCompilation>,
            ProductRef,
            Arc<FrontNativeProducts>,
        ),
        String,
    > {
        let (product, compilation) = self
            .completed
            .get()
            .ok_or("ownership bootstrap has not completed")?;
        if *product != selected {
            return Err("ownership bootstrap completion differs".into());
        }
        let prepared = self
            .prepared
            .get()
            .ok_or("ownership bootstrap has no selected source")?;
        Ok((
            prepared.compilation.clone(),
            *compilation,
            prepared.facts.clone(),
        ))
    }
    pub fn failure(&self, reference: Reference) -> Option<String> {
        self.failure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|(selected, message)| (*selected == reference).then(|| message.clone()))
    }
    fn fail(&self, message: String) -> Reference {
        if message.len() as u64 > self.weight.retained {
            return self.errors.resource;
        }
        let Some(reference) = self.source.store.put_content(FAILURE, message.as_bytes()) else {
            return self.errors.resource;
        };
        *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some((reference, message));
        reference
    }
    fn prepare(&self, selected: ProductRef) -> Result<&Prepared, String> {
        if let Some(prepared) = self.prepared.get() {
            return if prepared.source == selected {
                Ok(prepared)
            } else {
                Err("ownership source changed within immutable bootstrap".into())
            };
        }
        let facts = self
            .source
            .source
            .outcome(selected)
            .ok_or("ownership native source root is unavailable")?
            .products;
        let plan = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .ok_or("ownership bootstrap plan already consumed")?;
        let declarations = NativeDeclarations::from_settled(
            &facts.signatures,
            facts.roots.clone(),
            facts.catalogue.clone(),
            facts.statics.clone(),
            facts.extra.clone(),
        )
        .map_err(|e| format!("ownership native declarations: {e:?}"))?;
        let input = declarations.program.input.clone();
        let reader = facts
            .original_context
            .reader(facts.originals.clone(), self.source.store.clone())
            .map_err(|e| format!("ownership original products: {e:?}"))?;
        let inputs = NativeInstanceInputs::from_products(declarations, reader)
            .map_err(|e| format!("ownership native instance inputs: {e:?}"))?;
        let source_paths = inputs
            .declarations
            .sources
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        if source_paths
            .iter()
            .any(|path| !self.source.definitions.contains(path))
        {
            return Err("ownership source escaped admitted Lower namespace".into());
        }
        let front = native_lower::inputs(
            self.source.lower.clone(),
            facts.original_context.clone(),
            &source_paths,
        )?;
        let instances = InstanceTasks::new(
            inputs,
            self.source.store.clone(),
            plan.options,
            plan.policy,
            self.weight,
            plan.admission.instances,
        )?
        .with_store_verification(plan.verify_store)?
        .with_front_lower(front)?;
        let arena = ArenaTasks::new(
            instances.clone(),
            self.source.store.clone(),
            0,
            self.weight,
            plan.admission.arena,
        )?;
        let compilation = OwnershipCompilation::new(
            instances,
            arena,
            EmissionOptions::request(
                input,
                native::OptLevel::None,
                false,
                Default::default(),
                true,
                false,
                false,
            ),
            self.weight,
            plan.admission.compilation,
            plan.verify_store,
        )?;
        self.routing
            .bind(compilation.factories())
            .map_err(str::to_owned)?;
        self.prepared
            .set(Prepared {
                source: selected,
                facts,
                compilation,
            })
            .map_err(|_| "ownership bootstrap dispatch constructed twice")?;
        Ok(self.prepared.get().unwrap())
    }
    fn publish(&self, compilation: ProductRef) -> Result<ProductRef, String> {
        self.prepared
            .get()
            .ok_or("ownership dispatch unavailable")?
            .compilation
            .outcome(compilation)?;
        let bytes = serde_json::to_vec(&(
            compilation.address.0,
            compilation.action.0,
            compilation.record,
        ))
        .map_err(|e| e.to_string())?;
        let record = self
            .source
            .store
            .put_content(OUTPUT, &bytes)
            .ok_or("ownership bootstrap transport exhausted")?;
        if OUTPUT.reference(&bytes) != Some(record) {
            return Err("ownership bootstrap content differs".into());
        }
        let product = ProductRef {
            address: self.query.address,
            action: Action(ContentHash::of_parts(
                "hirsel.host.ownership-bootstrap-action.v1",
                [
                    self.query.address.0.0.as_slice(),
                    record.output.0.0.as_slice(),
                ],
            )),
            record,
        };
        self.source
            .store
            .associate_product(product.address, product.action, record);
        self.completed
            .set((product, compilation))
            .map_err(|_| "ownership bootstrap completed twice")?;
        Ok(product)
    }
}
struct Factory(Arc<OwnershipBootstrap>);
impl QueryFactory for Factory {
    fn create(&self, query: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if query != self.0.query {
            return Err(self.0.errors.protocol);
        }
        Ok(Box::new(Task {
            owner: self.0.clone(),
            terminal: None,
        }))
    }
}
struct Task {
    owner: Arc<OwnershipBootstrap>,
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
            let source = self.owner.source.source.query();
            match cx.read(source) {
                Poll::Pending => return QueryPoll::Waiting(vec![source]),
                Poll::Ready(Err(error)) => self.terminal = Some(Err(error)),
                Poll::Ready(Ok(product)) => match self.owner.prepare(product) {
                    Err(error) => self.terminal = Some(Err(self.owner.fail(error))),
                    Ok(prepared) => {
                        let query = prepared.compilation.query_id();
                        match cx.read(query) {
                            Poll::Pending => return QueryPoll::Waiting(vec![query]),
                            Poll::Ready(Err(error)) => self.terminal = Some(Err(error)),
                            Poll::Ready(Ok(product)) => {
                                self.terminal = Some(
                                    self.owner.publish(product).map_err(|e| self.owner.fail(e)),
                                )
                            }
                        }
                    }
                },
            }
        }
        match self.terminal.unwrap() {
            Ok(product) => QueryPoll::Complete(product),
            Err(error) => QueryPoll::Failed(error),
        }
    }
}
