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
    completed: Mutex<BTreeMap<Key, Outcome>>,
    anchors: Arc<crate::NativeRecordContext>,
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
    /// The immutable front already owns the address/definition index. Only
    /// demanded recipes need a task or a completed output in this owner.
    pub fn input_bytes(_front: &FrontBodyQueries) -> Option<u64> {
        Some(std::mem::size_of::<Self>() as u64)
    }
    /// The same shared-index owner before front construction.
    pub fn input_bytes_for_definitions<'a>(
        _definitions: impl IntoIterator<Item = &'a DefPath>,
    ) -> Option<u64> {
        Some(std::mem::size_of::<Self>() as u64)
    }
    fn address(input: Key) -> Key {
        ContentHash::of_parts("hirsel.front.storage.address.v1", [input.0.as_slice()])
    }
    pub fn query(&self, path: &DefPath) -> Option<QueryId> {
        let input = self.front.query(path)?;
        Some(QueryId {
            kind: FRONT_STORAGE_KIND,
            address: hirsel_artifact_store::Address(Self::address(input.address.0)),
        })
    }
    fn definition(&self, id: QueryId) -> Option<DefId> {
        if id.kind != FRONT_STORAGE_KIND {
            return None;
        }
        self.front.inputs.iter().find_map(|(address, input)| {
            (Self::address(*address) == id.address.0).then_some(input.definition)
        })
    }
    // A composed recipe uses its parent's execution grant. Its retained
    // products and the bounded task frame remain charged to this owner.
    pub(crate) fn fits_poll(&self, parent: MemoryWeight) -> bool {
        self.weight.scratch <= parent.scratch && self.weight.stack <= parent.stack
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
        let definition = self.0.definition(id).ok_or(self.0.errors.protocol)?;
        let terminal = self
            .0
            .completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id.address.0)
            .map(|outcome| outcome.product);
        Ok(Box::new(StorageTask {
            owner: self.0.clone(),
            id,
            definition,
            admitted: false,
            terminal,
        }))
    }
}
struct StorageTask {
    owner: Arc<FrontStorageQueries>,
    id: QueryId,
    definition: DefId,
    admitted: bool,
    terminal: Option<Result<ProductRef, Reference>>,
}
const _: () = assert!(std::mem::size_of::<StorageTask>() + 2 * std::mem::size_of::<usize>() <= 512);
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
        let path = self.definition.path();
        let Some(front) = self.owner.front.query(path) else {
            return finish(Err(self.owner.errors.protocol));
        };
        let selected = match cx.read(front) {
            Poll::Pending => return QueryPoll::Waiting(vec![front]),
            Poll::Ready(Err(reference)) => Err(reference),
            Poll::Ready(Ok(product)) => {
                if self.owner.front.outcome(product.record).is_some() {
                    self.run(product)
                } else {
                    let Some(body) = self.owner.front.body_query(path) else {
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
        let path = self.definition.path();
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
            DefId::intern(path),
            &owner.front.citations,
            &mut reads,
        );
        let result = (|| {
            let signature = provider.signature(DefId::intern(path))?;
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
            .encode(&*owner.anchors, &(path, &result, &*reads))
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
        let inventory =
            FrontStorageQueries::input_bytes(&front).ok_or(QueryError::NotImplemented)?;
        if inventory > allowance.bytes() {
            return Err(QueryError::NotImplemented);
        }
        let source = front.stage_source();
        Ok(Arc::new(FrontStorageQueries {
            front,
            completed: Mutex::new(BTreeMap::new()),
            anchors: source.anchors,
            records: source.records,
            weight,
            errors,
            retention: Mutex::new(allowance.bytes() - inventory),
            executions: std::sync::atomic::AtomicU64::new(0),
            _allowance: allowance,
        }))
    }
}
