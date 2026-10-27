//! Certified execution is a request dependency of ordinary Codegen and Link.
//! The complete C-admitted closure binds both planners and final object checks.
use super::{root_tasks::NativeRootTasks, *};
use crate::native::{ReadyProgram, object_queries::CodegenKey};
use hirsel_engine::{
    runtime::*,
    store::{Action, Address, Domain},
};
use std::{collections::BTreeMap, sync::OnceLock, task::Poll};

pub const EXECUTION_KIND: u32 = 0x4e00_0031;
pub const EXECUTION_OBJECTS_KIND: u32 = 0x4e00_0032;
const PLAN: Domain = Domain {
    kind: EXECUTION_KIND,
    schema: 1,
};
const OBJECTS: Domain = Domain {
    kind: EXECUTION_OBJECTS_KIND,
    schema: 1,
};

pub struct ExecutionProduct {
    pub product: ProductRef,
    pub environments: BTreeMap<ContentHash, Option<hirsel_lir::LayoutId>>,
    pub binding: hirsel_lir::products::Binding,
    root: ProductRef,
    reads: Vec<ProductRef>,
    pub ahead: hirsel_lir::ahead::AheadPlan,
    pub continuation: hirsel_lir::continuation::ContinuationPlans,
}
pub struct ExecutionTasks {
    roots: Arc<NativeRootTasks>,
    query: QueryId,
    objects: BTreeMap<ContentHash, CodegenKey>,
    weight: MemoryWeight,
    completed: OnceLock<ProductRef>,
}
impl ExecutionTasks {
    pub(super) fn retained_dispatch_bytes(&mut self) -> u64 {
        let mut weight = hirsel_engine::NativeMetadataWeight::default();
        weight.add(size_of::<Self>() + 2 * size_of::<usize>());
        weight.packed_map(&mut self.objects, |weight, _, key| {
            super::ownership_compilation::metadata::address(weight, &key.owner);
            weight.add(key.backend.capacity());
        });
        weight.bytes()
    }
    pub fn new(
        roots: Arc<NativeRootTasks>,
        keys: &[CodegenKey],
        weight: MemoryWeight,
    ) -> Arc<Self> {
        let query = QueryId {
            kind: EXECUTION_KIND,
            address: Address(ContentHash::of_parts(
                "hirsel.native.execution-address.v1",
                [roots.query_id().address.0.0.as_slice()],
            )),
        };
        Arc::new(Self {
            roots,
            query,
            objects: keys
                .iter()
                .map(|key| (Self::objects_id(key).address.0, key.clone()))
                .collect(),
            weight,
            completed: OnceLock::new(),
        })
    }
    pub fn query_id(&self) -> QueryId {
        self.query
    }
    pub fn objects_id(key: &CodegenKey) -> QueryId {
        QueryId {
            kind: EXECUTION_OBJECTS_KIND,
            address: Address(ContentHash::of_parts(
                "hirsel.native.execution-objects-address.v1",
                [key.address().0.0.as_slice()],
            )),
        }
    }
    fn publish(
        &self,
        query: QueryId,
        domain: Domain,
        action: Action,
        bytes: &[u8],
    ) -> Result<ProductRef, String> {
        let store = &self.roots.native.snapshot.store;
        if self.roots.native.snapshot.verify_store == hirsel_engine::store::VerifyStore::Recompute
            && let Some(previous) = store
                .lookup_product(query.address)
                .filter(|p| p.action == action)
            && domain.reference(bytes) != Some(previous.reference)
        {
            return Err("recomputed execution product differs from cached output".into());
        }
        let record = store
            .put_content(domain, bytes)
            .ok_or("execution publication exceeds admission")?;
        if domain.reference(bytes) != Some(record) {
            return Err("execution content identity differs".into());
        }
        store.associate_product(query.address, action, record);
        Ok(ProductRef {
            address: query.address,
            action,
            record,
        })
    }
    fn plan(&self, root: ProductRef, reads: &[ProductRef]) -> Result<ProductRef, String> {
        let (mut native, selected) = self.roots.products(root, reads)?;
        let snapshot = &self.roots.native.snapshot;
        let mut owners = snapshot.owners.values().cloned().collect::<Vec<_>>();
        owners.sort_by(InstanceAddress::canonical_cmp);
        let products = owners
            .iter()
            .map(|owner| native.ready(owner))
            .collect::<Result<Vec<_>, _>>()?;
        let (units, table) = native.with_ready(&products, |units, inputs| {
            Ok((
                units.iter().map(|u| (*u).clone()).collect::<Vec<_>>(),
                inputs.layouts.clone(),
            ))
        })?;
        let mut program =
            native.with_link_context(&selected, &products, |program| Ok(program.clone()))?;
        if program.table != table {
            return Err("execution Ready and Link bindings differ".into());
        }
        program.units = units;
        program.inline_instances = snapshot.emission.inline_instances.clone();
        program.ahead_environments = hirsel_lir::ahead::environments(
            &program
                .units
                .iter()
                .map(|u| u.unit().clone())
                .collect::<Vec<_>>(),
        );
        let closures = products
            .iter()
            .map(|p| {
                let bytes = snapshot
                    .store
                    .get_content(p.layout_closure)
                    .ok_or("missing execution layouts")?;
                hirsel_codec::from_text::<hirsel_lir::products::LayoutClosure>(
                    std::str::from_utf8(&bytes).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.message)
            })
            .collect::<Result<Vec<_>, String>>()?;
        let binding = hirsel_lir::products::Binding::new(
            &program.target,
            &closures.iter().collect::<Vec<_>>(),
        )?;
        if binding.table != program.table {
            return Err("execution binding differs from admitted Ready".into());
        }
        let ahead = crate::native::ahead::plan(&program);
        if !hirsel_lir::ahead::validate_cpu_workers(
            &program.units.iter().collect::<Vec<_>>(),
            &program.table,
            &program.target,
            &ahead,
            program.fuel,
        ) {
            return Err("invalid CPU ahead certificate".into());
        }
        let continuation = crate::native::continuation::plans(&program);
        crate::native::continuation::validate(&program, &continuation)
            .map_err(|e| e.to_string())?;
        let dependencies = reads.iter().map(|p| p.record).collect::<Vec<_>>();
        let bytes = serde_json::to_vec(&(ahead.identity, continuation.identity, snapshot.context))
            .map_err(|e| e.to_string())?;
        let action = Action(ContentHash::of_parts(
            "hirsel.native.execution-action.v1",
            [
                serde_json::to_vec(&(root.record, dependencies, snapshot.context))
                    .map_err(|e| e.to_string())?
                    .as_slice(),
            ],
        ));
        let product = self.publish(self.query, PLAN, action, &bytes)?;
        // Bodies belong only to a demanded planner/emitter/checker scope.
        // Keep one layout binding and capture declarations with the plans.
        snapshot
            .execution
            .set(Arc::new(ExecutionProduct {
                product,
                environments: program.ahead_environments,
                binding,
                root,
                reads: reads.to_vec(),
                ahead,
                continuation,
            }))
            .map_err(|_| "execution plans published twice")?;
        self.completed
            .set(product)
            .map_err(|_| "execution output published twice")?;
        Ok(product)
    }
    pub(super) fn with_program<R>(
        &self,
        use_program: impl FnOnce(&ReadyProgram) -> Result<R, String>,
    ) -> Result<R, String> {
        let execution = self
            .roots
            .native
            .snapshot
            .execution
            .get()
            .ok_or("execution plans have not settled")?;
        let (mut native, root) = self.roots.products(execution.root, &execution.reads)?;
        let products = execution
            .reads
            .iter()
            .map(|read| {
                super::ready_product(self.roots.native.snapshot.store.as_ref(), *read).map(Arc::new)
            })
            .collect::<Result<Vec<_>, _>>()?;
        native.with_execution_context(&root, &products, use_program)
    }
    fn emit(&self, query: QueryId, plan: ProductRef) -> Result<ProductRef, String> {
        let snapshot = &self.roots.native.snapshot;
        snapshot
            .execution
            .get()
            .filter(|p| p.product == plan)
            .ok_or("unread execution plan")?;
        let key = self
            .objects
            .get(&query.address.0)
            .ok_or("unregistered execution backend")?;
        self.with_program(|program| {
            let store = snapshot.store.as_ref();
            let action = Action(ContentHash::of_parts(
                "hirsel.native.execution-objects-action.v1",
                [
                    plan.record.output.0.0.as_slice(),
                    snapshot.context.output.0.0.as_slice(),
                ],
            ));
            if snapshot.verify_store == hirsel_engine::store::VerifyStore::Off
                && let Some(previous) = store
                    .lookup_product(query.address)
                    .filter(|p| p.action == action)
            {
                let product = ProductRef {
                    address: query.address,
                    action,
                    record: previous.reference,
                };
                if let Ok(objects) = self.load_objects(product) {
                    crate::native::ahead::check_objects(program, &objects)
                        .map_err(|e| e.to_string())?;
                    return Ok(product);
                }
            }
            let backend = match key.backend.as_str() {
                "llvm" => crate::native::Backend::Llvm,
                "cranelift" => crate::native::Backend::Cranelift,
                _ => return Err("unknown execution backend".into()),
            };
            let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                &program.table,
                &program.finalizers,
            );
            let mut objects = ahead_objects(program, backend, snapshot.emission.opt, &registry)
                .map_err(|e| e.to_string())?;
            objects.extend(
                continuation_objects(
                    program,
                    backend,
                    snapshot.emission.opt,
                    snapshot.emission.measurement,
                )
                .map_err(|e| e.to_string())?,
            );
            crate::native::ahead::check_objects(program, &objects).map_err(|e| e.to_string())?;
            // These are checked before transport, then checked again from exact bytes at Link.
            let certificates = crate::native::continuation::ahead_certificates(program, &objects);
            if certificates.len()
                != objects
                    .iter()
                    .find_map(|o| o.ahead.as_ref())
                    .map_or(0, |e| e.bounds.len())
            {
                return Err("ahead objects lack continuation certificates".into());
            }
            let text = hirsel_codec::to_text(&objects);
            let blob = crate::native::object_queries::store_link_input(store, text.as_bytes())?;
            let bytes = serde_json::to_vec(&blob).map_err(|e| e.to_string())?;
            let product = self.publish(query, OBJECTS, action, &bytes)?;
            Ok(product)
        })
    }
    pub(super) fn load_objects(
        &self,
        product: ProductRef,
    ) -> Result<Vec<hirsel_lir::stage::ObjectArtifact>, String> {
        self.objects_from_reference(product.record)
    }
    pub(super) fn objects_from_reference(
        &self,
        record: Reference,
    ) -> Result<Vec<hirsel_lir::stage::ObjectArtifact>, String> {
        if record.domain != OBJECTS {
            return Err("wrong execution objects domain".into());
        }
        let store = self.roots.native.snapshot.store.as_ref();
        let bytes = store
            .get_content(record)
            .ok_or("missing execution object descriptor")?;
        let blob = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let bytes = crate::native::object_queries::link_input_bytes(store, blob)?;
        hirsel_codec::from_text(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)
            .map_err(|e| e.message)
    }
}
pub struct Factory(pub Arc<ExecutionTasks>);
impl QueryFactory for Factory {
    fn create(&self, query: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if query != self.0.query
            && !(query.kind == EXECUTION_OBJECTS_KIND
                && self.0.objects.contains_key(&query.address.0))
        {
            return Err(self.0.roots.native.failure("unregistered execution query"));
        }
        Ok(Box::new(Task {
            tasks: self.0.clone(),
            query,
            terminal: None,
        }))
    }
}
struct Task {
    tasks: Arc<ExecutionTasks>,
    query: QueryId,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.query
    }
    fn weight(&self) -> MemoryWeight {
        self.tasks.weight
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if self.terminal.is_none() {
            let ids = if self.query.kind == EXECUTION_KIND {
                let mut owners = self
                    .tasks
                    .roots
                    .native
                    .snapshot
                    .owners
                    .values()
                    .collect::<Vec<_>>();
                owners.sort_by(|a, b| a.canonical_cmp(b));
                std::iter::once(self.tasks.roots.query_id())
                    .chain(owners.iter().map(|o| o.ready_query_id()))
                    .collect::<Vec<_>>()
            } else {
                vec![self.tasks.query]
            };
            let mut reads = Vec::new();
            let mut waiting = Vec::new();
            for id in ids {
                match cx.read(id) {
                    Poll::Pending => waiting.push(id),
                    Poll::Ready(Ok(p)) => reads.push(p),
                    Poll::Ready(Err(e)) => {
                        self.terminal = Some(Err(e));
                        break;
                    }
                }
            }
            if self.terminal.is_none() {
                if !waiting.is_empty() {
                    return QueryPoll::Waiting(waiting);
                }
                let result = if self.query.kind == EXECUTION_KIND {
                    self.tasks.plan(reads[0], &reads[1..])
                } else {
                    self.tasks.emit(self.query, reads[0])
                };
                self.terminal = Some(result.map_err(|e| self.tasks.roots.native.failure(&e)));
            }
        }
        match self.terminal.unwrap() {
            Ok(p) => QueryPoll::Complete(p),
            Err(e) => QueryPoll::Failed(e),
        }
    }
}

use crate::native::ahead::{body_frame, inputs, plan, unknown_machine_calls};
use crate::native::{NativeError, StageName};
use hirsel_lir::{ahead::AheadPlan, continuation::ContinuationPlans};

/// One owner for each alternate definition and immutable root record, independent
/// of ordinary partitions, cached unit hits, and installed standard images.
fn ahead_objects(
    program: &ReadyProgram,
    backend: crate::native::Backend,
    opt: hirsel_lir::emission::OptLevel,
    registry: &hirsel_lir::descriptor_registry::DescriptorRegistry,
) -> Result<Vec<hirsel_lir::stage::ObjectArtifact>, NativeError> {
    use std::collections::BTreeMap;
    let plan = plan(program);
    if plan.roots.is_empty() {
        return Ok(Vec::new());
    }
    let all: Vec<_> = program.units.iter().collect();
    let units: Vec<_> = all
        .into_iter()
        .filter(|u| plan.bodies.contains_key(&u.unit().header.instance))
        .collect();
    let plans = crate::native::continuation::plans(program);
    let inputs = inputs(program, &plan, &plans, opt);
    let failed = |message: String| NativeError::new(StageName::Emission, message);
    let emitted = match backend {
        crate::native::Backend::Llvm => {
            hirsel_llvm::emit_ahead_object(&units, &inputs).map_err(|e| e.to_string())
        }
        crate::native::Backend::Cranelift => {
            hirsel_cranelift::emit_ahead_object(&units, &inputs).map_err(|e| e.to_string())
        }
    };
    let (mut object, frames) = emitted.map_err(failed)?;
    let frames: BTreeMap<_, _> = plan
        .bodies
        .keys()
        .filter_map(|id| Some((*id, body_frame(&plan, &plans, *id, &frames)?)))
        .collect();
    // Backend-created memcpy/division/outlined helpers have no private-work
    // summary. Refuse publication even if their ordinary use is permitted.
    let machine_helpers = unknown_machine_calls(&object, &plan, &plans, program);
    let bounds: BTreeMap<_, _> = plan
        .roots
        .iter()
        .filter_map(|(id, root)| {
            if machine_helpers {
                return None;
            }
            let [startup, getter, handoff, depth] = crate::native::continuation::AHEAD_HELPER_BYTES;
            hirsel_lir::ahead::stack_bound_with_depth(
                &root.graph,
                &frames,
                startup,
                getter,
                root.handoff_bytes(handoff),
                depth,
            )
            .map(|n| (*id, n))
        })
        .collect();
    let mut metadata = hirsel_cranelift::emit_ahead_metadata(&inputs, &plan, &bounds, registry)
        .map_err(|e| failed(e.to_string()))?;
    let evidence = hirsel_lir::ahead::MachineEvidence {
        plan: plan.identity,
        object: ContentHash::of_bytes(&object.bytes),
        options: object.reproducibility.options,
        frames: frames.into_iter().collect(),
        bounds: bounds.into_iter().collect(),
    };
    object.ahead = Some(evidence.clone());
    metadata.ahead = Some(evidence);
    Ok(vec![object, metadata])
}

/// Every resumable plan's state alternates and dispatcher, in one object.
fn continuation_objects(
    program: &ReadyProgram,
    backend: crate::native::Backend,
    opt: hirsel_lir::emission::OptLevel,
    measurement: bool,
) -> Result<Vec<hirsel_lir::stage::ObjectArtifact>, NativeError> {
    let plans = crate::native::continuation::plans(program);
    if !plans.resumable() {
        return Ok(Vec::new());
    }
    // Entered-runtime certificates include incoming callable provenance from
    // callers that may themselves remain stackful. Emitters check this full
    // set, then emit alternates only for Resumable bodies.
    let units = program.units.iter().collect::<Vec<_>>();
    let mut ahead = crate::native::ahead::plan(program);
    if !hirsel_lir::ahead::validate_cpu_workers(
        &units,
        &program.table,
        &program.target,
        &ahead,
        program.fuel,
    ) {
        return Err(NativeError::new(
            StageName::Emission,
            "invalid CPU ahead certificate",
        ));
    }
    ahead
        .spawns
        .retain(|_, root| ahead.roots.get(root).is_some_and(|root| root.cpu.is_some()));
    static EMPTY: std::collections::BTreeSet<ContentHash> = std::collections::BTreeSet::new();
    let inputs = hirsel_lir::emission::EmitInputs {
        ahead: Some(&ahead),
        continuation: Some(&plans),
        alternate: false,
        index: None,
        input: &program.input,
        target: &program.target,
        layouts: &program.table,
        keys: &program.keys,
        manifest: &program.manifest,
        statics: &program.statics,
        inline_instances: &program.inline_instances,
        available_externally: &EMPTY,
        callbacks: !program.callbacks.is_empty(),
        tracking: program.tracking(),
        foreign: &program.foreign_dispatch,
        opt,
        measurement,
        fuel: program.fuel,
        verify_backend: program.validation == crate::native::Validation::EveryStage,
    };
    let object = match backend {
        crate::native::Backend::Cranelift => {
            hirsel_cranelift::emit_resume_object(&units, &inputs, &plans)
                .map_err(|e| NativeError::new(StageName::Emission, e.to_string()))?
        }
        crate::native::Backend::Llvm => hirsel_llvm::emit_resume_object(&units, &inputs, &plans)
            .map_err(|e| NativeError::new(StageName::Emission, e.to_string()))?,
    };
    Ok(vec![object])
}

/// Shared execution-object producer. Ordinary batch diagnostics use these same
/// certified objects; only the request graph owns production planning and reuse.
pub(in crate::native) fn objects(
    program: &ReadyProgram,
    backend: crate::native::Backend,
    opt: hirsel_lir::emission::OptLevel,
    measurement: bool,
    registry: &hirsel_lir::descriptor_registry::DescriptorRegistry,
) -> Result<Vec<hirsel_lir::stage::ObjectArtifact>, NativeError> {
    let mut objects = ahead_objects(program, backend, opt, registry)?;
    objects.extend(continuation_objects(program, backend, opt, measurement)?);
    Ok(objects)
}
