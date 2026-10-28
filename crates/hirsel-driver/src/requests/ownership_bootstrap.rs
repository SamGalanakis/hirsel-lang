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
    root_source::{NativeLinkFacts, NativeLinkSource},
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
    queries::instance::CALLABLE_VALUE_KIND,
    queries::instance_tasks::CALLABLE_GRAPH_KIND,
    queries::instance_tasks::INSTANCE_INPUT_KIND,
    queries::instance_tasks::DISCOVERY_KIND,
    queries::ARENA_CANDIDATES_KIND,
    queries::ARENA_CHOSEN_KIND,
    queries::arena_tasks::ARENA_GRAPH_KIND,
    queries::ownership_compilation::OWNERSHIP_COMPILATION_KIND,
    queries::seal_demand::SEAL_DEMAND_KIND,
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
    pub fuel: bool,
    pub policy: ContentHash,
    pub verify_store: VerifyStore,
    pub admission: Admission,
}
struct Prepared {
    source: ProductRef,
    facts: Arc<FrontNativeProducts>,
    compilation: Arc<OwnershipCompilation>,
    link: Arc<NativeLinkSource>,
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
    output_allowance: Arc<OutputAllowance>,
}
/// One bounded original grant, reusable after a diagnostic consumer drops its
/// objects. Dropping a lease returns capacity to this owner, not a larger cap.
struct OutputAllowance {
    available: Mutex<u64>,
    _allowance: MemoryReservation,
}
pub(super) struct OutputReservation {
    owner: Arc<OutputAllowance>,
    bytes: u64,
}
impl OutputAllowance {
    fn new(allowance: MemoryReservation) -> Arc<Self> {
        Arc::new(Self {
            available: Mutex::new(allowance.bytes()),
            _allowance: allowance,
        })
    }
    fn reserve(self: &Arc<Self>, bytes: u64) -> Result<OutputReservation, String> {
        let mut available = self
            .available
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *available = available
            .checked_sub(bytes)
            .ok_or_else(|| {
                format!(
                    "ownership output metadata exceeds original admission: {bytes} bytes requested, {} available",
                    *available
                )
            })?;
        Ok(OutputReservation {
            owner: self.clone(),
            bytes,
        })
    }
}
impl Drop for OutputReservation {
    fn drop(&mut self) {
        let mut available = self
            .owner
            .available
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *available += self.bytes;
    }
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
        if !plan.options.discharge.0.is_empty() {
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
        metadata.add(
            std::mem::size_of::<Self>()
                + std::mem::size_of::<OutputAllowance>()
                + 5 * std::mem::size_of::<usize>(),
        );
        metadata.vec(&source.definitions, |w, path| w.path(path));
        if metadata.bytes() > allowance.bytes()
            || weight.retained < std::mem::size_of::<Task>() as u64
        {
            return Err("ownership bootstrap metadata exceeds original admission".into());
        }
        // Diagnostic outputs retain both native objects and checked marker
        // bindings. Split their share from the existing compilation grant;
        // the compiler must fit its correspondingly smaller remaining share.
        let output_allowance = plan
            .admission
            .compilation
            .split(plan.admission.compilation.bytes() / 5)
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
            output_allowance: OutputAllowance::new(output_allowance),
        }))
    }
    pub fn query(&self) -> QueryId {
        self.query
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    pub fn reserve_output(&self, bytes: u64) -> Result<OutputReservation, String> {
        self.output_allowance.reserve(bytes)
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
    pub fn link_source(&self) -> Result<Arc<NativeLinkSource>, String> {
        self.prepared
            .get()
            .map(|prepared| prepared.link.clone())
            .ok_or("ownership source has not completed".into())
    }
    pub fn failure(&self, reference: Reference) -> Option<String> {
        self.failure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|(selected, message)| (*selected == reference).then(|| message.clone()))
            .or_else(|| self.prepared.get()?.compilation.failure(reference))
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
        let mut plan = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .ok_or("ownership bootstrap plan already consumed")?;
        let declarations = NativeDeclarations::from_front(facts.clone())
            .map_err(|e| format!("ownership native declarations: {e:?}"))?;
        let input = declarations.program.input.clone();
        let reader = facts
            .original_context
            .reader(
                facts.originals.iter().map(|p| (**p).clone()),
                self.source.store.clone(),
            )
            .map_err(|e| format!("ownership original products: {e:?}"))?;
        let entries = declarations
            .context
            .sources
            .iter()
            .filter(|(path, _)| {
                declarations.program.signatures.get(*path).is_some_and(|signature| {
                    matches!(&signature.kind, hirsel_types::SignatureKind::Function(function) if function.generics.is_empty())
                })
            })
            .map(|(path, context)| (path.clone(), context.entry.0))
            .collect();
        let callees =
            native::settled_callee_metadata(facts.run.callees.clone(), &facts.run.runtime)
                .map_err(|error| error.to_string())?;
        let link = Arc::new(
            NativeLinkSource::from_settled(
                NativeLinkFacts {
                    program: &declarations.program,
                    operations: Arc::new(declarations.program.operation_registry()),
                    roots: &declarations.roots,
                    declarations: &declarations.sources,
                    entries: &entries,
                    anchors: &declarations.source_anchors,
                    signatures: facts.signatures.clone(),
                    semantic: facts.semantic.clone(),
                    selected: std::slice::from_ref(&facts.run.entry),
                    callees: Some(callees),
                },
                &reader,
            )
            .map_err(|error| error.to_string())?,
        );
        link.retain_metadata(&mut plan.admission.instances)?;
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
                link.foreign_dispatch(),
                true,
                false,
                plan.fuel,
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
                link,
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

#[cfg(test)]
mod output_tests {
    use super::*;
    #[test]
    fn diagnostic_output_grant_recycles_released_objects_without_raising_admission() {
        let budget = ProcessBudget::new(64);
        let owner = OutputAllowance::new(budget.reserve(64).unwrap());
        let first = owner.reserve(48).unwrap();
        assert!(
            owner.reserve(17).is_err(),
            "live objects retain the exact bound"
        );
        drop(first);
        let second = owner.reserve(64).unwrap();
        assert!(owner.reserve(1).is_err());
        assert_eq!(budget.reserved(), 64);
        drop(owner);
        assert_eq!(
            budget.reserved(),
            64,
            "the surviving consumer keeps its original owner"
        );
        drop(second);
        assert_eq!(budget.reserved(), 0);
    }
}
