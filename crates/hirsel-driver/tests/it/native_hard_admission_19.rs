//! The hard-admission boundary replacing UnitObjects' obsolete soft throttle.
use hirsel_engine::{runtime::*, store::*};
use hirsel_intern::jobs::{COMPILER_STACK_BYTES, Jobs};
use hirsel_syntax::ContentHash;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Factory {
    product: ProductRef,
    polls: Arc<AtomicUsize>,
    weight: MemoryWeight,
}
struct Task {
    id: QueryId,
    product: ProductRef,
    polls: Arc<AtomicUsize>,
    weight: MemoryWeight,
}
impl QueryFactory for Factory {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        Ok(Box::new(Task {
            id,
            product: self.product.clone(),
            polls: self.polls.clone(),
            weight: self.weight,
        }))
    }
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.id
    }
    fn weight(&self) -> MemoryWeight {
        self.weight
    }
    fn poll(&mut self, _: &mut dyn QueryContext) -> QueryPoll {
        self.polls.fetch_add(1, Ordering::SeqCst);
        QueryPoll::Complete(self.product.clone())
    }
}
#[test]
fn one_byte_short_refuses_with_resource_error_and_exact_need_executes() {
    let store = MemStore::default();
    let diagnostic = |text: &[u8]| {
        store
            .put_content(
                Domain {
                    kind: 0x7a000001,
                    schema: 1,
                },
                text,
            )
            .unwrap()
    };
    let errors = RuntimeErrors {
        resource: diagnostic(b"typed resource admission refusal"),
        cycle: diagnostic(b"cycle"),
        cancelled: diagnostic(b"cancelled"),
        protocol: diagnostic(b"protocol"),
        panicked: diagnostic(b"panicked"),
    };
    let address = Address(ContentHash::of_bytes(b"hard-admission-boundary"));
    let query = QueryId {
        kind: 0x7a000002,
        address,
    };
    let product = crate::scheduled_front::imported_fixture(
        &store,
        address,
        Action(address.0),
        diagnostic(b"completed"),
    );
    let weight = MemoryWeight {
        retained: 1024,
        scratch: 4096,
        stack: 0,
    };
    let need = COMPILER_STACK_BYTES as u64
        + AdmittedRuntime::query_capacity_bytes(1).unwrap()
        + weight.retained
        + weight.scratch;
    for bytes in [need - 1, need] {
        let budget = ProcessBudget::new(need);
        let polls = Arc::new(AtomicUsize::new(0));
        let mut runtime = AdmittedRuntime::with_query_capacity(
            Jobs::ONE,
            budget.reserve(bytes).unwrap(),
            errors,
            1,
        )
        .unwrap();
        runtime
            .register(
                query.kind,
                Arc::new(Factory {
                    product: product.clone(),
                    polls: polls.clone(),
                    weight,
                }),
            )
            .unwrap();
        let request = runtime.request(&[query]);
        let result = runtime.wait(request);
        if bytes < need {
            assert_eq!(result, Err(errors.resource));
            assert_eq!(
                polls.load(Ordering::SeqCst),
                0,
                "oversized singleton was polled"
            );
        } else {
            assert_eq!(result, Ok(vec![product.clone()]));
            assert_eq!(polls.load(Ordering::SeqCst), 1);
        }
        assert_eq!(
            budget.reserved(),
            bytes,
            "runtime changed its process grant"
        );
        runtime.shutdown();
        drop(runtime);
        assert_eq!(budget.reserved(), 0);
    }
}
