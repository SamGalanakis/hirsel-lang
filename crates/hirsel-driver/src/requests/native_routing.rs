//! Install native query kinds before the runtime's first request. The selected
//! source continuation attaches one complete factory set after its Run read;
//! routing itself can neither compile nor reconstruct a source controller.
use hirsel_engine::runtime::{MemoryReservation, QueryFactory, QueryId, QueryTask};
use hirsel_engine::store::Reference;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, OnceLock},
};

pub(super) struct NativeRouting {
    kinds: BTreeSet<u32>,
    bound: OnceLock<BTreeMap<u32, Arc<dyn QueryFactory>>>,
    failure: Reference,
    _allowance: MemoryReservation,
}
impl NativeRouting {
    pub fn new(
        kinds: &[u32],
        failure: Reference,
        allowance: MemoryReservation,
    ) -> Result<Arc<Self>, &'static str> {
        // Includes both indexes, Arc factory slots and their tree nodes. The
        // factories own their independently admitted compiler product scopes.
        let bytes = (kinds.len() as u64)
            .checked_mul(512)
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>() as u64))
            .ok_or("native routing inventory overflow")?;
        if kinds.is_empty() || bytes > allowance.bytes() {
            return Err("native routing inventory exceeds admission");
        }
        let kinds_set = kinds.iter().copied().collect::<BTreeSet<_>>();
        if kinds_set.len() != kinds.len() {
            return Err("native routing has duplicate query kinds");
        }
        Ok(Arc::new(Self {
            kinds: kinds_set,
            bound: OnceLock::new(),
            failure,
            _allowance: allowance,
        }))
    }
    pub fn factories(self: &Arc<Self>) -> Vec<(u32, Arc<dyn QueryFactory>)> {
        let factory: Arc<dyn QueryFactory> = self.clone();
        self.kinds
            .iter()
            .map(|kind| (*kind, factory.clone()))
            .collect()
    }
    /// Commit the complete selected set once, so no child can observe partial
    /// dispatch. Producers must attach before reading any routed child query.
    pub fn bind(&self, factories: Vec<(u32, Arc<dyn QueryFactory>)>) -> Result<(), &'static str> {
        let count = factories.len();
        let factories = factories.into_iter().collect::<BTreeMap<_, _>>();
        if factories.len() != count
            || factories.len() != self.kinds.len()
            || factories.keys().ne(self.kinds.iter())
        {
            return Err("native factory set differs from installed query kinds");
        }
        self.bound
            .set(factories)
            .map_err(|_| "native factories are already attached")
    }
}
impl QueryFactory for NativeRouting {
    fn create(&self, query: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        self.bound
            .get()
            .and_then(|factories| factories.get(&query.kind))
            .ok_or(self.failure)?
            .create(query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_engine::runtime::*;
    use hirsel_engine::store::{Action, Address, Domain};
    use hirsel_intern::jobs::Jobs;
    use hirsel_syntax::ContentHash;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        task::Poll,
    };

    fn query(kind: u32) -> QueryId {
        QueryId {
            kind,
            address: Address(ContentHash::of_bytes(&kind.to_le_bytes())),
        }
    }
    fn product(query: QueryId) -> ProductRef {
        crate::fixture_product_bytes(
            query.address,
            Action(query.address.0),
            Domain { kind: 1, schema: 1 },
            &query.address.0.0,
        )
    }
    struct Leaf(Arc<AtomicUsize>);
    impl QueryFactory for Leaf {
        fn create(&self, query: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(LeafTask(query)))
        }
    }
    struct LeafTask(QueryId);
    impl QueryTask for LeafTask {
        fn id(&self) -> QueryId {
            self.0
        }
        fn weight(&self) -> MemoryWeight {
            MemoryWeight {
                retained: 256,
                scratch: 256,
                stack: 0,
            }
        }
        fn poll(&mut self, _: &mut dyn QueryContext) -> QueryPoll {
            QueryPoll::Complete(product(self.0))
        }
    }
    struct Bootstrap {
        routing: Arc<NativeRouting>,
        leaf: Arc<Leaf>,
    }
    impl QueryFactory for Bootstrap {
        fn create(&self, _: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
            Ok(Box::new(BootstrapTask {
                routing: self.routing.clone(),
                leaf: self.leaf.clone(),
                bound: false,
            }))
        }
    }
    struct BootstrapTask {
        routing: Arc<NativeRouting>,
        leaf: Arc<Leaf>,
        bound: bool,
    }
    impl QueryTask for BootstrapTask {
        fn id(&self) -> QueryId {
            query(1)
        }
        fn weight(&self) -> MemoryWeight {
            MemoryWeight {
                retained: 1024,
                scratch: 1024,
                stack: 0,
            }
        }
        fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
            match cx.read(query(2)) {
                Poll::Pending => return QueryPoll::Waiting(vec![query(2)]),
                Poll::Ready(Err(error)) => return QueryPoll::Failed(error),
                Poll::Ready(Ok(_)) => {}
            }
            if !self.bound {
                self.routing.bind(vec![(3, self.leaf.clone())]).unwrap();
                self.bound = true;
            }
            match cx.read(query(3)) {
                Poll::Pending => QueryPoll::Waiting(vec![query(3)]),
                Poll::Ready(Err(error)) => QueryPoll::Failed(error),
                Poll::Ready(Ok(_)) => QueryPoll::Complete(product(query(1))),
            }
        }
    }
    #[test]
    fn native_routing_attaches_only_after_source_dependency_and_before_child_creation() {
        let budget = ProcessBudget::new(256 << 20);
        let failure = Domain { kind: 1, schema: 1 }
            .reference(b"protocol")
            .unwrap();
        let routing = NativeRouting::new(&[3], failure, budget.reserve(4096).unwrap()).unwrap();
        let leaf = Arc::new(Leaf(Arc::new(AtomicUsize::new(0))));
        assert!(matches!(routing.create(query(3)), Err(error) if error == failure));
        assert!(routing.bind(vec![]).is_err());
        assert!(routing.bind(vec![(4, leaf.clone())]).is_err());
        assert!(
            routing
                .bind(vec![(3, leaf.clone()), (3, leaf.clone())])
                .is_err()
        );
        let errors = RuntimeErrors {
            resource: failure,
            cycle: failure,
            cancelled: failure,
            protocol: failure,
            panicked: failure,
        };
        let mut runtime =
            AdmittedRuntime::new(Jobs::ONE, budget.reserve(192 << 20).unwrap(), errors).unwrap();
        for (kind, factory) in routing.factories() {
            runtime.register(kind, factory).unwrap();
        }
        runtime
            .register(
                1,
                Arc::new(Bootstrap {
                    routing: routing.clone(),
                    leaf: leaf.clone(),
                }),
            )
            .unwrap();
        runtime
            .register(2, Arc::new(Leaf(Arc::new(AtomicUsize::new(0)))))
            .unwrap();
        let first = runtime.request(&[query(1)]);
        let second = runtime.request(&[query(1)]);
        assert!(
            runtime.register(4, leaf.clone()).is_err(),
            "query registry freezes at the first request"
        );
        runtime.completion().wait(first);
        assert_eq!(
            runtime.poll(first),
            Poll::Ready(Ok(vec![product(query(1))]))
        );
        assert_eq!(
            runtime.poll(second),
            Poll::Ready(Ok(vec![product(query(1))]))
        );
        assert_eq!(leaf.0.load(Ordering::SeqCst), 1);
        assert!(
            routing.bind(vec![(3, leaf)]).is_err(),
            "attached factories cannot be replaced"
        );
        assert!(matches!(routing.create(query(4)), Err(error) if error == failure));
        drop(runtime);
        assert_eq!(budget.reserved(), 4096);
        drop(routing);
        assert_eq!(budget.reserved(), 0);
    }
}
