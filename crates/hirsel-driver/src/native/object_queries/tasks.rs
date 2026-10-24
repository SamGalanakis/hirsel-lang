//! Typed Codegen/Link continuations. Creation observes immutable key metadata
//! only; actual dependency reads suspend before any backend/linker is entered.
use super::*;
use hirsel_engine::runtime::{
    MemoryWeight, QueryContext, QueryFactory, QueryId, QueryPoll, QueryTask,
};
use std::task::Poll;

pub const CODEGEN_KIND: u32 = 0x4f000001;
pub const LINK_KIND: u32 = 0x4f000002;
impl CodegenKey {
    pub fn query_id(&self) -> QueryId {
        QueryId {
            kind: CODEGEN_KIND,
            address: self.address(),
        }
    }
}
impl LinkKey {
    pub fn query_id(&self) -> QueryId {
        QueryId {
            kind: LINK_KIND,
            address: self.address(),
        }
    }
}

/// Source/native discovery produces the manifest query. Creating a Link task
/// does not enumerate bodies or rebuild a whole-program native plan.
#[derive(Clone, Debug)]
pub enum ObjectTaskRequest {
    Codegen {
        key: CodegenKey,
        ready: QueryId,
        weight: MemoryWeight,
    },
    Link {
        key: LinkKey,
        manifest: QueryId,
        weight: MemoryWeight,
    },
}
impl ObjectTaskRequest {
    fn id(&self) -> QueryId {
        match self {
            Self::Codegen { key, .. } => key.query_id(),
            Self::Link { key, .. } => key.query_id(),
        }
    }
    fn weight(&self) -> MemoryWeight {
        match self {
            Self::Codegen { weight, .. } | Self::Link { weight, .. } => *weight,
        }
    }
}

/// Complete dependencies described by a settled native discovery/link manifest.
/// Preserve archive/foreign order. Fragment order is canonicalized by Link.
#[derive(Clone, Debug)]
pub struct LinkDependencies {
    pub fragments: Vec<QueryId>,
    /// Native metadata consumed by Link assembly, read explicitly even on a
    /// Link hit. These products convey no proof; the native view re-admits C.
    pub native_reads: Vec<QueryId>,
    pub static_manifest: QueryId,
    pub runtime_archives: Vec<QueryId>,
    pub foreign_inputs: Vec<QueryId>,
    pub launch_manifest: QueryId,
}

/// Installed by the engine for one immutable request snapshot. Implementations
/// resolve descriptors from the one store; descriptor resolution grants no
/// native/V authority. `products` creates task-local producer state restricted
/// to the settled reads, never a mutable global controller behind a mutex.
pub trait ObjectTaskSnapshot: Send + Sync {
    fn store(&self) -> &dyn ResultStore;
    /// Bounded metadata lookup only: no dependency execution or decoding bodies.
    fn request(&self, id: QueryId) -> Result<ObjectTaskRequest, Reference>;
    fn ready_descriptor(&self, product: ProductRef) -> Result<ReadyProduct, Reference>;
    /// Resolve each optimizer read's registered native query kind and address.
    fn optimizer_query(&self, product: ProductRef) -> Result<QueryId, Reference>;
    fn link_dependencies(&self, manifest: ProductRef) -> Result<LinkDependencies, Reference>;
    /// Resolve a settled static/runtime/foreign/launch record to its content.
    fn link_input(&self, product: ProductRef) -> Result<Reference, Reference>;
    fn products(&self, settled: &[ProductRef]) -> Result<Box<dyn ObjectProducts>, Reference>;
    fn failure(&self, message: &str) -> Reference;
}

pub struct ObjectTaskFactory<S> {
    snapshot: Arc<S>,
}
impl<S: ObjectTaskSnapshot> ObjectTaskFactory<S> {
    pub fn new(snapshot: Arc<S>) -> Self {
        Self { snapshot }
    }
}
impl<S: ObjectTaskSnapshot + 'static> QueryFactory for ObjectTaskFactory<S> {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        let request = self.snapshot.request(id)?;
        if request.id() != id {
            return Err(self.snapshot.failure("object task key/address mismatch"));
        }
        Ok(Box::new(ObjectTask {
            snapshot: self.snapshot.clone(),
            request,
            terminal: None,
        }))
    }
}
struct ObjectTask<S> {
    snapshot: Arc<S>,
    request: ObjectTaskRequest,
    terminal: Option<Result<ProductRef, Reference>>,
}

// Read every dependency in the prescribed order even when another is pending.
// The runtime records real edges/coalesces wakeups; no worker blocks here.
fn read(cx: &mut dyn QueryContext, ids: &[QueryId]) -> Result<Vec<ProductRef>, QueryPoll> {
    let mut waiting = Vec::new();
    let mut products = Vec::with_capacity(ids.len());
    for id in ids {
        match cx.read(*id) {
            Poll::Pending => {
                if !waiting.contains(id) {
                    waiting.push(*id);
                }
            }
            Poll::Ready(Ok(product)) => products.push(product),
            Poll::Ready(Err(error)) => return Err(QueryPoll::Failed(error)),
        }
    }
    if waiting.is_empty() {
        Ok(products)
    } else {
        Err(QueryPoll::Waiting(waiting))
    }
}

impl<S: ObjectTaskSnapshot> ObjectTask<S> {
    fn compute(&self, cx: &mut dyn QueryContext) -> Result<ProductRef, QueryPoll> {
        let failed = QueryPoll::Failed;
        let product = match &self.request {
            ObjectTaskRequest::Codegen { key, ready, .. } => {
                let own = read(cx, &[*ready])?[0];
                let descriptor = self.snapshot.ready_descriptor(own).map_err(failed)?;
                if descriptor.product != own {
                    return Err(failed(self.snapshot.failure("Ready descriptor mismatch")));
                }
                let ids = descriptor
                    .optimizer_reads
                    .iter()
                    .map(|p| self.snapshot.optimizer_query(*p))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(failed)?;
                let copies = read(cx, &ids)?;
                if copies
                    .iter()
                    .zip(&descriptor.optimizer_reads)
                    .any(|(current, old)| {
                        current.address != old.address || current.record != old.record
                    })
                {
                    return Err(failed(
                        self.snapshot.failure("stale optimizer body dependency"),
                    ));
                }
                let mut settled = vec![own];
                settled.extend(copies);
                let mut products = self.snapshot.products(&settled).map_err(failed)?;
                products
                    .codegen(key)
                    .map_err(|e| failed(self.snapshot.failure(&e)))?
                    .product
            }
            ObjectTaskRequest::Link { key, manifest, .. } => {
                let manifest = read(cx, &[*manifest])?[0];
                let deps = self.snapshot.link_dependencies(manifest).map_err(failed)?;
                let mut ids = deps.fragments.clone();
                ids.push(deps.static_manifest);
                ids.extend(&deps.runtime_archives);
                ids.extend(&deps.foreign_inputs);
                ids.push(deps.launch_manifest);
                ids.extend(&deps.native_reads);
                let outputs = read(cx, &ids)?;
                let n = deps.fragments.len();
                let fragments = outputs[..n]
                    .iter()
                    .map(|p| {
                        fragment_product(self.snapshot.store(), *p)
                            .map_err(|e| self.snapshot.failure(&e))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(failed)?;
                let resolve = |p| self.snapshot.link_input(p).map_err(failed);
                let static_manifest = resolve(outputs[n])?;
                let r = n + 1;
                let f = r + deps.runtime_archives.len();
                let l = f + deps.foreign_inputs.len();
                let runtime_archives = outputs[r..f]
                    .iter()
                    .map(|p| resolve(*p))
                    .collect::<Result<_, _>>()?;
                let foreign_inputs = outputs[f..l]
                    .iter()
                    .map(|p| resolve(*p))
                    .collect::<Result<_, _>>()?;
                let inputs = LinkInputs {
                    fragments,
                    static_manifest,
                    runtime_archives,
                    foreign_inputs,
                    launch_manifest: resolve(outputs[l])?,
                };
                let mut settled = vec![manifest];
                settled.extend(outputs);
                let mut products = self.snapshot.products(&settled).map_err(failed)?;
                products
                    .link(key, &inputs)
                    .map_err(|e| failed(self.snapshot.failure(&e)))?
                    .product
            }
        };
        if product.address != self.request.id().address {
            return Err(failed(
                self.snapshot
                    .failure("object task returned another product"),
            ));
        }
        Ok(product)
    }
}
impl<S: ObjectTaskSnapshot> QueryTask for ObjectTask<S> {
    fn id(&self) -> QueryId {
        self.request.id()
    }
    fn weight(&self) -> MemoryWeight {
        self.request.weight()
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if let Some(result) = self.terminal {
            return match result {
                Ok(p) => QueryPoll::Complete(p),
                Err(e) => QueryPoll::Failed(e),
            };
        }
        match self.compute(cx) {
            Ok(product) => {
                self.terminal = Some(Ok(product));
                QueryPoll::Complete(product)
            }
            Err(QueryPoll::Failed(error)) => {
                self.terminal = Some(Err(error));
                QueryPoll::Failed(error)
            }
            Err(other) => other,
        }
    }
}
