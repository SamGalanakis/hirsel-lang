//! Oracle artifacts depend on the accepted export and ordinary native producers.
use super::{host_selector, native_assembly::NativeAssembly, proof_export::ProofExport};
use crate::native::{Backend, object_queries};
use hirsel_engine::{CheckCompletion, ToolError, runtime::*, store::*};
use hirsel_syntax::ContentHash;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    task::Poll,
};

pub(super) const BYTES: u64 = 4096;
pub(super) const KIND: u32 = 0x5200_0040;
const FAILURE: Domain = Domain {
    kind: 0x5200_0042,
    schema: 1,
};
const OUTPUT: Domain = Domain {
    kind: 0x5200_0041,
    schema: 1,
};
struct Completed {
    product: ProductRef,
    artifacts: BTreeMap<String, Vec<u8>>,
    _allowance: MemoryReservation,
}
pub(super) struct NativeArtifacts {
    query: QueryId,
    pub assembly: Arc<NativeAssembly>,
    export: Arc<ProofExport>,
    budget: Arc<ProcessBudget>,
    store: Arc<dyn ResultStore>,
    errors: RuntimeErrors,
    weight: MemoryWeight,
    _allowance: MemoryReservation,
    completed: Mutex<Option<Completed>>,
    failure: Mutex<Option<(Reference, String)>>,
}
impl NativeArtifacts {
    pub fn new(
        assembly: Arc<NativeAssembly>,
        export: Arc<ProofExport>,
        budget: Arc<ProcessBudget>,
        store: Arc<dyn ResultStore>,
        errors: RuntimeErrors,
        weight: MemoryWeight,
        allowance: MemoryReservation,
    ) -> Arc<Self> {
        Arc::new(Self {
            query: QueryId {
                kind: KIND,
                address: Address(ContentHash::of_parts(
                    "hirsel.native-artifacts.address.v1",
                    [
                        assembly.query().address.0.0.as_slice(),
                        export.query().address.0.0.as_slice(),
                    ],
                )),
            },
            assembly,
            export,
            budget,
            store,
            errors,
            weight,
            _allowance: allowance,
            completed: Mutex::new(None),
            failure: Mutex::new(None),
        })
    }
    fn fail(&self, error: String) -> Reference {
        let Some(reference) = self.store.put_content(FAILURE, error.as_bytes()) else {
            return self.errors.resource;
        };
        *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some((reference, error));
        reference
    }
    pub fn failure(&self, reference: Reference) -> Option<ToolError> {
        self.failure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|(selected, _)| *selected == reference)
            .map(|(_, error)| ToolError::Runtime(error.clone()))
            .or_else(|| self.assembly.failure(reference))
    }
    pub fn query(&self) -> QueryId {
        self.query
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    pub fn outcome(&self, product: ProductRef) -> Result<BTreeMap<String, Vec<u8>>, ToolError> {
        let completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        completed
            .as_ref()
            .filter(|c| c.product == product)
            .map(|c| c.artifacts.clone())
            .ok_or_else(|| ToolError::Runtime("native artifacts product is unavailable".into()))
    }
}
struct Factory(Arc<NativeArtifacts>);
impl QueryFactory for Factory {
    fn create(&self, query: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if query != self.0.query {
            return Err(self.0.errors.protocol);
        }
        Ok(Box::new(Task(self.0.clone())))
    }
}
struct Task(Arc<NativeArtifacts>);
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.0.query
    }
    fn weight(&self) -> MemoryWeight {
        self.0.weight
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        let owner = &self.0;
        if let Some(completed) = owner
            .completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            return QueryPoll::Complete(completed.product);
        }
        let exported = match cx.read(owner.export.query()) {
            Poll::Pending => return QueryPoll::Waiting(vec![owner.export.query()]),
            Poll::Ready(Err(error)) => return QueryPoll::Failed(error),
            Poll::Ready(Ok(product)) => product,
        };
        let Ok((check, tir)) = owner.export.outcome(exported) else {
            return QueryPoll::Failed(owner.errors.protocol);
        };
        let accepted = check.completion == CheckCompletion::Complete
            && !check
                .diagnostics
                .iter()
                .any(|d| d.severity == hirsel_diag::Severity::Error);
        let mut native = None;
        if accepted {
            let query = owner.assembly.query();
            let built = match cx.read(query) {
                Poll::Pending => return QueryPoll::Waiting(vec![query]),
                Poll::Ready(Err(error)) => Err(owner
                    .assembly
                    .failure(error)
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| format!("native compiler refusal: {error:?}"))),
                Poll::Ready(Ok(product)) => owner.assembly.bootstrap().outcome(product),
            };
            match built {
                Err(error) => {
                    native = Some(host_selector::NativeArtifacts {
                        receipts: format!("build: {error}\n").into_bytes(),
                        objects: vec![],
                    })
                }
                Ok((compilation, product, _source)) => {
                    let Ok(tasks) = compilation.native(product) else {
                        return QueryPoll::Failed(owner.errors.protocol);
                    };
                    let mut reads = Vec::new();
                    let mut waiting = Vec::new();
                    for query in tasks.receipt_queries() {
                        match cx.read(query) {
                            Poll::Pending => waiting.push(query),
                            Poll::Ready(Err(error)) => return QueryPoll::Failed(error),
                            Poll::Ready(Ok(product)) if product.address == query.address => {
                                reads.push(product)
                            }
                            _ => return QueryPoll::Failed(owner.errors.protocol),
                        }
                    }
                    if !waiting.is_empty() {
                        return QueryPoll::Waiting(waiting);
                    }
                    let result = (|| {
                        let receipts = tasks.receipts(&reads)?;
                        let count = compilation.emitted_instances(product)?;
                        let outputs = compilation.outputs(product)?;
                        if count == 0 || outputs.len() != 2 * count {
                            return Err("oracle native backend outputs are incomplete".into());
                        }
                        let objects = [Backend::Llvm, Backend::Cranelift]
                            .into_iter()
                            .enumerate()
                            .map(|(index, backend)| {
                                let objects = outputs[index * count..(index + 1) * count]
                                    .iter()
                                    .map(|product| {
                                        let fragment = object_queries::fragment_product(
                                            owner.store.as_ref(),
                                            *product,
                                        )?;
                                        object_queries::load_object(
                                            owner.store.as_ref(),
                                            fragment.object,
                                        )
                                        .map(|object| object.bytes)
                                    })
                                    .collect();
                                (backend, objects)
                            })
                            .collect();
                        Ok::<_, String>(host_selector::NativeArtifacts { receipts, objects })
                    })();
                    match result {
                        Ok(outputs) => native = Some(outputs),
                        Err(error) => return QueryPoll::Failed(owner.fail(error)),
                    }
                }
            }
        }
        let artifacts = host_selector::completed_artifacts(&check, tir, native);
        let bytes = artifacts.iter().try_fold(0u64, |n, (key, value)| {
            n.checked_add((key.capacity() + value.capacity() + 128) as u64)
        });
        let Some(bytes) = bytes.and_then(|n| n.checked_mul(2)) else {
            return QueryPoll::Failed(owner.errors.resource);
        };
        let Ok(allowance) = owner.budget.reserve(bytes) else {
            return QueryPoll::Failed(owner.errors.resource);
        };
        let mut descriptor = Vec::new();
        for (name, bytes) in &artifacts {
            descriptor.extend_from_slice(&(name.len() as u64).to_le_bytes());
            descriptor.extend_from_slice(name.as_bytes());
            descriptor.extend_from_slice(&ContentHash::of_bytes(bytes).0);
        }
        let Some(record) = owner.store.put_content(OUTPUT, &descriptor) else {
            return QueryPoll::Failed(owner.errors.resource);
        };
        let action = Action(ContentHash::of_parts(
            "hirsel.native-artifacts.action.v1",
            [
                owner.query.address.0.0.as_slice(),
                record.output.0.0.as_slice(),
            ],
        ));
        let product = ProductRef {
            address: owner.query.address,
            action,
            record,
        };
        *owner.completed.lock().unwrap_or_else(|e| e.into_inner()) = Some(Completed {
            product,
            artifacts,
            _allowance: allowance,
        });
        owner
            .store
            .associate_product(product.address, action, record);
        QueryPoll::Complete(product)
    }
}
