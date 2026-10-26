//! Demand-driven source storage equations from immutable checked P products.
//! The worklist consumer requests these bodies; signature lookup never invokes
//! storage derivation or a synchronous compiler/controller fallback.
use super::*;
use hirsel_types::storage::graph::StorageDefinition;
use std::task::Poll;
pub const FRONT_STORAGE_KIND: u32 = 0x4600_000d;
const OUTPUT: Domain = Domain {
    kind: 0x4600_000e,
    schema: 1,
};
const FAILURE: Domain = Domain {
    kind: 0x4600_000f,
    schema: 1,
};
struct Outcome {
    product: Result<ProductRef, Reference>,
    result: Result<Arc<QueryOutput<StorageDefinition>>, QueryError>,
    reads: Arc<Reads>,
}
pub struct FrontStorageQueries {
    front: Arc<FrontBodyQueries>,
    inputs: BTreeMap<Key, DefPath>,
    completed: Mutex<BTreeMap<Key, Outcome>>,
    anchors: crate::NativeRecordContext,
    records: crate::NativeRecords,
    weight: MemoryWeight,
    errors: RuntimeErrors,
    retention: Mutex<u64>,
    executions: std::sync::atomic::AtomicU64,
    _allowance: MemoryReservation,
}
#[derive(Clone)]
pub struct FrontStorageOutput {
    pub output: Arc<QueryOutput<StorageDefinition>>,
    pub reads: Arc<Reads>,
    _owner: Arc<FrontStorageQueries>,
}
impl FrontStorageQueries {
    /// Potential recipe address/path slots only; actual recipe demands debit
    /// their existing output bound from the remainder of this reservation.
    pub fn input_bytes(front: &FrontBodyQueries) -> Option<u64> {
        front.inputs.values().try_fold(0u64, |n, input| {
            n.checked_add((std::mem::size_of::<(Key, DefPath)>() + 64) as u64)?
                .checked_add(bincode::serialized_size(&input.definition).ok()?)
        })
    }

    pub fn query(&self, path: &DefPath) -> Option<QueryId> {
        self.inputs.iter().find_map(|(&address, selected)| {
            (path == selected).then_some(QueryId {
                kind: FRONT_STORAGE_KIND,
                address: hirsel_artifact_store::Address(address),
            })
        })
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(StorageFactory(self.clone()))
    }
    pub fn outcome(self: &Arc<Self>, product: ProductRef) -> Option<FrontStorageOutput> {
        let completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        let selected = completed.get(&product.address.0)?;
        if selected.product != Ok(product) {
            return None;
        }
        Some(FrontStorageOutput {
            output: selected.result.as_ref().ok()?.clone(),
            reads: selected.reads.clone(),
            _owner: self.clone(),
        })
    }
    pub fn failure(&self, reference: Reference) -> Option<QueryError> {
        self.completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .find(|outcome| outcome.product == Err(reference))
            .and_then(|outcome| outcome.result.as_ref().err().cloned())
    }
    pub fn executions(&self) -> u64 {
        self.executions.load(std::sync::atomic::Ordering::Relaxed)
    }
}
struct StorageFactory(Arc<FrontStorageQueries>);
impl QueryFactory for StorageFactory {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        let path = self
            .0
            .inputs
            .get(&id.address.0)
            .filter(|_| id.kind == FRONT_STORAGE_KIND)
            .ok_or(self.0.errors.protocol)?
            .clone();
        Ok(Box::new(StorageTask {
            owner: self.0.clone(),
            id,
            path,
            admitted: false,
            terminal: None,
        }))
    }
}
struct StorageTask {
    owner: Arc<FrontStorageQueries>,
    id: QueryId,
    path: DefPath,
    admitted: bool,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for StorageTask {
    fn id(&self) -> QueryId {
        self.id
    }
    fn weight(&self) -> MemoryWeight {
        self.owner.weight
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if let Some(terminal) = self.terminal {
            return finish(terminal);
        }
        if !self.admitted {
            let bound = self
                .owner
                .weight
                .retained
                .saturating_mul(2)
                .saturating_add(512);
            let mut retention = self
                .owner
                .retention
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if *retention < bound {
                self.terminal = Some(Err(self.owner.errors.resource));
                return finish(self.terminal.unwrap());
            }
            *retention -= bound;
            self.admitted = true;
        }
        let Some(front) = self.owner.front.query(&self.path) else {
            return finish(Err(self.owner.errors.protocol));
        };
        let selected = match cx.read(front) {
            Poll::Pending => return QueryPoll::Waiting(vec![front]),
            Poll::Ready(Err(reference)) => Err(reference),
            Poll::Ready(Ok(product)) => {
                if self.owner.front.outcome(product.record).is_some() {
                    self.run(product)
                } else {
                    let Some(body) = self.owner.front.body_query(&self.path) else {
                        return finish(Err(self.owner.errors.protocol));
                    };
                    match cx.read(body) {
                        Poll::Pending => return QueryPoll::Waiting(vec![body]),
                        Poll::Ready(Err(error)) => Err(error),
                        Poll::Ready(Ok(product)) => self.run(product),
                    }
                }
            }
        };
        self.terminal = Some(selected);
        finish(selected)
    }
}
impl StorageTask {
    fn run(&self, product: ProductRef) -> Result<ProductRef, Reference> {
        let owner = &self.owner;
        let body = owner
            .front
            .outcome(product.record)
            .ok_or(owner.errors.protocol)?;
        let analyzed = body
            .outcome
            .result
            .as_ref()
            .map_err(|_| owner.errors.protocol)?;
        let mut reads = ReadsBuilder::new();
        reads.merge(&body.outcome.reads);
        let mut provider = FrozenProvider::for_definition(
            &owner.front.snapshot,
            DefId::intern(&self.path),
            &owner.front.citations,
            &mut reads,
        );
        let result = (|| {
            let signature = provider.signature(DefId::intern(&self.path))?;
            let modes = owner
                .front
                .modes
                .as_ref()
                .ok_or(QueryError::NotImplemented)?;
            owner
                .executions
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let output = modes.storage(
                &owner.front.types,
                &owner.front.memo,
                &analyzed.value,
                &signature,
                &mut provider,
            )?;
            if has_errors(&output.diagnostics) {
                return Err(QueryError::Diagnostics(output.diagnostics));
            }
            Ok(Arc::new(output))
        })();
        if let Ok(output) = &result {
            for dependency in &output.dependencies {
                reads.record(
                    ReadKey::Signature(dependency.definition),
                    dependency.fingerprint,
                );
            }
        }
        let reads = Arc::new(reads.finish().map_err(|_| owner.errors.protocol)?);
        let bytes = owner
            .records
            .encode(&owner.anchors, &(&self.path, &result, &*reads))
            .ok_or(owner.errors.resource)?;
        if bytes.len() as u64 > owner.weight.retained {
            return Err(owner.errors.resource);
        }
        let failed = result.is_err();
        let record = owner
            .front
            .store
            .put_content(if failed { FAILURE } else { OUTPUT }, &bytes)
            .ok_or(owner.errors.resource)?;
        let action = hirsel_artifact_store::Action(ContentHash::of_parts(
            "hirsel.front.storage.action.v1",
            [
                product.action.0.0.as_slice(),
                crate::store::payload_reads_hash(&reads).0.as_slice(),
                record.output.0.0.as_slice(),
            ],
        ));
        let selected = if failed {
            Err(record)
        } else {
            Ok(ProductRef {
                address: self.id.address,
                action,
                record,
            })
        };
        owner
            .completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                self.id.address.0,
                Outcome {
                    product: selected,
                    result,
                    reads,
                },
            );
        if let Ok(product) = selected {
            owner
                .front
                .store
                .associate_product(product.address, product.action, product.record);
        }
        selected
    }
}
impl QueryEngine {
    pub fn front_storage_queries(
        &mut self,
        front: Arc<FrontBodyQueries>,
        weight: MemoryWeight,
        allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<FrontStorageQueries>, QueryError> {
        let mut inputs = BTreeMap::new();
        let mut inventory = 0u64;
        for (address, input) in &front.inputs {
            inventory = inventory
                .checked_add((std::mem::size_of::<(Key, DefPath)>() + 64) as u64)
                .and_then(|n| n.checked_add(bincode::serialized_size(&input.definition).ok()?))
                .ok_or(QueryError::NotImplemented)?;
            if inventory > allowance.bytes() {
                return Err(QueryError::NotImplemented);
            }
            let address =
                ContentHash::of_parts("hirsel.front.storage.address.v1", [address.0.as_slice()]);
            inputs.insert(address, input.definition.path().clone());
        }
        let store = front.store.clone();
        Ok(Arc::new(FrontStorageQueries {
            front,
            inputs,
            completed: Mutex::new(BTreeMap::new()),
            anchors: crate::NativeRecordContext::capture(self),
            records: crate::NativeRecords::new(self.types.clone(), store),
            weight,
            errors,
            retention: Mutex::new(allowance.bytes() - inventory),
            executions: std::sync::atomic::AtomicU64::new(0),
            _allowance: allowance,
        }))
    }
}
