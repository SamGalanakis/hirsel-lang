//! The native root is a suspending producer, never factory-time work.
use super::{roots::RootProducts, tasks::NativeTasks, *};
use crate::native::root_source::NativeLinkSource;
use hirsel_engine::runtime::*;
use std::{
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    task::Poll,
};

/// One immutable root request and its compact completed output. The OnceLock
/// carries ordinary descriptor data only, not C/V authority or another mutable
/// query controller. All cold instance/SCC production still precedes NativeTasks.
pub struct NativeRootTasks {
    pub(super) native: Arc<NativeTasks>,
    source: Arc<NativeLinkSource>,
    key: NativeRootKey,
    query: QueryId,
    reads: Vec<(QueryId, Reference)>,
    weight: MemoryWeight,
    completed: OnceLock<Arc<NativeRootProduct>>,
    pub publications: AtomicU64,
}
impl NativeRootTasks {
    pub(super) fn retained_dispatch_bytes(&self) -> u64 {
        let mut w = hirsel_engine::NativeMetadataWeight::default();
        w.add(size_of::<Self>() + 2 * size_of::<usize>());
        w.vec(&self.key.roots, |w, path| w.path(path));
        w.flat_vec(&self.reads);
        // The root later owns one canonical copy of the selected instances.
        w.add(size_of::<NativeRootProduct>() + 2 * size_of::<usize>());
        w.add(self.native.snapshot.owners.len().max(4) * size_of::<InstanceAddress>());
        for key in self.native.snapshot.owners.values() {
            super::ownership_compilation::metadata::address(&mut w, key);
        }
        w.bytes()
    }
    pub fn new(
        native: Arc<NativeTasks>,
        source: Arc<NativeLinkSource>,
        key: NativeRootKey,
        weight: MemoryWeight,
    ) -> Result<Arc<Self>, String> {
        if std::mem::size_of::<RootTask>() as u64 > weight.retained {
            return Err("root continuation exceeds declared retained allowance".into());
        }
        // Pure input validation: no native query and no product publication.
        RootProducts::new(native.snapshot.clone(), source.clone(), key.clone())?;
        let query = key.query_id()?;
        let mut reads = native
            .snapshot
            .instances
            .completed_inputs()?
            .into_iter()
            .map(|(key, result)| {
                (
                    QueryId {
                        kind: INSTANCE_KIND,
                        address: key.address(INSTANCE_KIND),
                    },
                    result.product.product.record,
                )
            })
            .collect::<Vec<_>>();
        reads.sort_by_key(|(key, _)| key.address.0);
        if (std::mem::size_of::<RootTask>()
            + reads.len() * std::mem::size_of::<(QueryId, Reference)>()) as u64
            > weight.scratch
        {
            return Err("root reads exceed declared scratch allowance".into());
        }
        Ok(Arc::new(Self {
            native,
            source,
            key,
            query,
            reads,
            weight,
            completed: OnceLock::new(),
            publications: AtomicU64::new(0),
        }))
    }

    /// Create a task-local scope from this producer's actual settled output and
    /// the Link task's explicit native reads. This runs no producer/dependency.
    pub fn products(
        &self,
        root: ProductRef,
        ready: &[ProductRef],
    ) -> Result<(RootProducts, Arc<NativeRootProduct>), String> {
        let selected = self.output(root)?;
        let view = RootProducts::settled(
            self.native.snapshot.clone(),
            self.source.clone(),
            self.key.clone(),
            selected.clone(),
            ready,
        )?;
        Ok((view, selected.clone()))
    }
    pub(super) fn output(&self, root: ProductRef) -> Result<Arc<NativeRootProduct>, String> {
        let selected = self
            .completed
            .get()
            .ok_or("root continuation has not completed")?;
        if selected.product != root {
            return Err("root continuation returned another output".into());
        }
        Ok(selected.clone())
    }
    pub fn query_id(&self) -> QueryId {
        self.query
    }
}

pub struct NativeRootFactory {
    inputs: Arc<NativeRootTasks>,
}
impl NativeRootFactory {
    pub fn new(inputs: Arc<NativeRootTasks>) -> Self {
        Self { inputs }
    }
}
impl QueryFactory for NativeRootFactory {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if id != self.inputs.query {
            return Err(self.inputs.native.failure("unregistered native root task"));
        }
        Ok(Box::new(RootTask {
            inputs: self.inputs.clone(),
            terminal: None,
        }))
    }
}
struct RootTask {
    inputs: Arc<NativeRootTasks>,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for RootTask {
    fn id(&self) -> QueryId {
        self.inputs.query
    }
    fn weight(&self) -> MemoryWeight {
        self.inputs.weight
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if let Some(result) = self.terminal {
            return match result {
                Ok(p) => QueryPoll::Complete(p),
                Err(e) => QueryPoll::Failed(e),
            };
        }
        let mut waiting = Vec::new();
        for (key, expected) in &self.inputs.reads {
            match cx.read(*key) {
                Poll::Pending => waiting.push(*key),
                Poll::Ready(Ok(product))
                    if product.address == key.address && product.record == *expected => {}
                result => {
                    let error = match result {
                        Poll::Ready(Err(error)) => error,
                        _ => self
                            .inputs
                            .native
                            .failure("native root input changed within immutable snapshot"),
                    };
                    self.terminal = Some(Err(error));
                    return QueryPoll::Failed(error);
                }
            }
        }
        if !waiting.is_empty() {
            return QueryPoll::Waiting(waiting);
        }
        let result = (|| {
            let mut products = RootProducts::new(
                self.inputs.native.snapshot.clone(),
                self.inputs.source.clone(),
                self.inputs.key.clone(),
            )?;
            let root = products.root(&self.inputs.key)?;
            if let Err(other) = self.inputs.completed.set(root.clone())
                && self.inputs.completed.get().unwrap().product != other.product
            {
                return Err("native root completion changed within immutable snapshot".into());
            }
            self.inputs.publications.fetch_add(1, Ordering::Relaxed);
            Ok(root.product)
        })()
        .map_err(|e: String| self.inputs.native.failure(&e));
        self.terminal = Some(result);
        match result {
            Ok(p) => QueryPoll::Complete(p),
            Err(e) => QueryPoll::Failed(e),
        }
    }
}
