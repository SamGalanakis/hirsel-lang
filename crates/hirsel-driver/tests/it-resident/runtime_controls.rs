//! Small immutable tasks exercising the real scheduler through the host adapter.
//! These are lifecycle controls, not compiler/verification/performance evidence.
use hirsel_artifact_store::{Action, Address, Domain};
use hirsel_driver::resident::{Host, RequestError};
use hirsel_engine::loader_inputs::{FrozenInputs, InputLimits, LoaderInputs};
use hirsel_engine::requests::{OutputReadiness, RequestEngine, SelectedRequest, observe_inputs};
use hirsel_engine::runtime::{
    AdmittedRuntime, MemoryWeight, ProcessBudget, ProductRef, QueryContext, QueryFactory, QueryId,
    QueryPoll, QueryTask, RuntimeErrors,
};
use hirsel_engine::store::{MemStore, ResultStore};
use hirsel_intern::jobs::Jobs;
use hirsel_syntax::ContentHash;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::task::Poll;
use std::time::{Duration, Instant};

const ALLOWANCE: u64 = (128 << 20) + (1 << 20);
fn root() -> QueryId {
    QueryId {
        kind: 123,
        address: Address(ContentHash::of_bytes(b"stable input root")),
    }
}
fn errors() -> RuntimeErrors {
    let r = Domain {
        kind: 999,
        schema: 1,
    }
    .reference(b"diagnostic")
    .unwrap();
    RuntimeErrors {
        resource: r,
        cycle: r,
        cancelled: r,
        protocol: r,
        panicked: r,
    }
}
struct Task {
    product: ProductRef,
    polls: Arc<AtomicUsize>,
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        root()
    }
    fn weight(&self) -> MemoryWeight {
        MemoryWeight {
            retained: 128,
            ..Default::default()
        }
    }
    fn poll(&mut self, _: &mut dyn QueryContext) -> QueryPoll {
        self.polls.fetch_add(1, Ordering::SeqCst);
        QueryPoll::Complete(self.product)
    }
}
struct Factory {
    product: ProductRef,
    polls: Arc<AtomicUsize>,
}
impl QueryFactory for Factory {
    fn create(&self, _: QueryId) -> Result<Box<dyn QueryTask>, hirsel_artifact_store::Reference> {
        Ok(Box::new(Task {
            product: self.product,
            polls: self.polls.clone(),
        }))
    }
}
struct Readiness {
    store: Arc<dyn ResultStore>,
}
impl OutputReadiness for Readiness {
    type Output = Vec<u8>;
    type Error = String;
    fn ready(&mut self, products: &[ProductRef]) -> Result<Vec<u8>, String> {
        let [product] = products else {
            return Err("wrong root count".into());
        };
        self.store
            .get_content(product.record)
            .map(|b| b.to_vec())
            .ok_or_else(|| "missing product".into())
    }
}
struct Engine {
    store: Arc<dyn ResultStore>,
    budget: Arc<ProcessBudget>,
    polls: Arc<AtomicUsize>,
    current: Option<(FrozenInputs, u8, Arc<Mutex<AdmittedRuntime>>)>,
}
impl RequestEngine for Engine {
    type Request = (PathBuf, u8);
    type Error = String;
    type Runtime = AdmittedRuntime;
    type Readiness = Readiness;
    fn select(
        &mut self,
        (path, policy): &Self::Request,
    ) -> Result<SelectedRequest<AdmittedRuntime, Readiness>, String> {
        let (bytes, inputs) = observe_inputs(
            InputLimits {
                bytes: 65536,
                operations: 100,
            },
            2,
            |c| c.read(path),
        )
        .map_err(|_| "input race")?;
        let bytes = bytes.map_err(|e| e.to_string())?;
        let same = self
            .current
            .as_ref()
            .is_some_and(|(old, p, _)| old == &inputs && p == policy);
        if !same {
            let mut parts = inputs.identity_parts();
            parts.push(vec![*policy]);
            let action = Action(ContentHash::of_parts(
                "hirsel.resident.fixture.action.v1",
                parts.iter().map(Vec::as_slice),
            ));
            let product = ProductRef {
                address: root().address,
                action,
                record: self
                    .store
                    .put_content(
                        Domain {
                            kind: 123,
                            schema: 1,
                        },
                        &bytes,
                    )
                    .unwrap(),
            };
            let reservation = self.budget.reserve(ALLOWANCE).map_err(|e| e.to_string())?;
            let mut runtime = AdmittedRuntime::new(Jobs::new(1), reservation, errors())
                .map_err(|e| e.to_string())?;
            runtime
                .register(
                    root().kind,
                    Arc::new(Factory {
                        product,
                        polls: self.polls.clone(),
                    }),
                )
                .unwrap();
            self.current = Some((inputs, *policy, Arc::new(Mutex::new(runtime))));
        }
        Ok(SelectedRequest {
            runtime: self.current.as_ref().unwrap().2.clone(),
            roots: vec![root()],
            output: Readiness {
                store: self.store.clone(),
            },
        })
    }
}
fn engine(
    budget: Arc<ProcessBudget>,
    store: Arc<dyn ResultStore>,
    polls: Arc<AtomicUsize>,
) -> Engine {
    Engine {
        budget,
        store,
        polls,
        current: None,
    }
}
fn complete<
    R: hirsel_engine::runtime::QueryRuntime,
    O: OutputReadiness<Output = Vec<u8>, Error = String>,
>(
    request: &mut hirsel_driver::resident::RequestLease<R, O>,
) -> Result<Vec<u8>, RequestError<String>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match request.poll() {
            Poll::Ready(result) => return result,
            Poll::Pending => {
                assert!(Instant::now() < deadline, "runtime request stalled");
                std::thread::yield_now();
            }
        }
    }
}
#[test]
fn actual_runtime_coalesces_sixteen_identical_requests_and_cancel_preserves_live_consumers() {
    let tree = tempfile::tempdir().unwrap();
    let source = tree.path().join("main.hir");
    std::fs::write(&source, b"snapshot").unwrap();
    let budget = ProcessBudget::new(2 * ALLOWANCE);
    let store = Arc::new(MemStore::default());
    let polls = Arc::new(AtomicUsize::new(0));
    let mut host = Host::new(engine(budget.clone(), store, polls.clone()));
    let mut requests = (0..16)
        .map(|_| host.request(&(source.clone(), 0)).unwrap())
        .collect::<Vec<_>>();
    drop(requests.remove(0));
    for request in &mut requests {
        assert_eq!(complete(request).unwrap(), b"snapshot");
    }
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    drop(requests);
    drop(host);
    assert_eq!(budget.reserved(), 0);
}
#[test]
fn actual_runtime_keeps_old_revision_and_refuses_cap_multiplication() {
    let tree = tempfile::tempdir().unwrap();
    let source = tree.path().join("main.hir");
    std::fs::write(&source, b"before").unwrap();
    let budget = ProcessBudget::new(2 * ALLOWANCE);
    let store = Arc::new(MemStore::default());
    let polls = Arc::new(AtomicUsize::new(0));
    let mut host = Host::new(engine(budget.clone(), store, polls));
    let mut before = host.request(&(source.clone(), 0)).unwrap();
    std::fs::write(&source, b"after!").unwrap();
    let mut after = host.request(&(source.clone(), 0)).unwrap();
    assert_eq!(budget.reserved(), 2 * ALLOWANCE);
    std::fs::write(&source, b"third!").unwrap();
    assert!(host.request(&(source.clone(), 0)).is_err());
    assert_eq!(complete(&mut before).unwrap(), b"before");
    assert_eq!(complete(&mut after).unwrap(), b"after!");
    drop(before);
    drop(after);
    drop(host);
    assert_eq!(budget.reserved(), 0);
}
#[test]
fn actual_runtime_policy_change_restart_and_zero_retention_use_ordinary_products() {
    let tree = tempfile::tempdir().unwrap();
    let source = tree.path().join("main.hir");
    std::fs::write(&source, b"payload").unwrap();
    let budget = ProcessBudget::new(2 * ALLOWANCE);
    let store = Arc::new(MemStore::default());
    let polls = Arc::new(AtomicUsize::new(0));
    let mut host = Host::new(engine(budget.clone(), store.clone(), polls.clone()));
    let mut request = host.request(&(source.clone(), 0)).unwrap();
    assert_eq!(complete(&mut request).unwrap(), b"payload");
    request.evict_to(0);
    drop(request);
    let mut request = host.request(&(source.clone(), 1)).unwrap();
    assert_eq!(complete(&mut request).unwrap(), b"payload");
    drop(request);
    drop(host);
    assert_eq!(budget.reserved(), 0);
    let mut fresh = Host::new(engine(budget.clone(), store, polls.clone()));
    let mut request = fresh.request(&(source, 1)).unwrap();
    assert_eq!(complete(&mut request).unwrap(), b"payload");
    assert_eq!(polls.load(Ordering::SeqCst), 3);
    drop(request);
    drop(fresh);
    assert_eq!(budget.reserved(), 0);
}
