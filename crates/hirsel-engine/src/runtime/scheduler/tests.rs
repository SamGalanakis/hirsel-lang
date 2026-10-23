use super::*;
use hirsel_artifact_store::Domain;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

fn query(n: u64) -> QueryId {
    QueryId {
        kind: 1,
        address: Address(ContentHash::of_bytes(&n.to_le_bytes())),
    }
}
fn reference(text: &[u8]) -> Reference {
    Domain { kind: 1, schema: 1 }.reference(text).unwrap()
}
fn product(q: QueryId) -> ProductRef {
    ProductRef {
        address: q.address,
        action: Action(q.address.0),
        record: reference(&q.address.0.0),
    }
}
fn errors() -> RuntimeErrors {
    RuntimeErrors {
        resource: reference(b"resource"),
        cycle: reference(b"cycle"),
        cancelled: reference(b"cancelled"),
        protocol: reference(b"protocol"),
        panicked: reference(b"panic"),
    }
}
const STACK: usize = 1 << 20;
fn runtime(jobs: usize, budget: u64) -> AdmittedRuntime {
    AdmittedRuntime::with_stack(Jobs::new(jobs), budget, errors(), STACK).unwrap()
}
#[derive(Clone)]
struct Spec {
    dependencies: Vec<QueryId>,
    weight: MemoryWeight,
    result: Option<Reference>,
}
impl Spec {
    fn deps(dependencies: &[u64]) -> Self {
        Self {
            dependencies: dependencies.iter().copied().map(query).collect(),
            weight: MemoryWeight {
                scratch: 32,
                retained: 16,
                stack: 0,
            },
            result: None,
        }
    }
}
struct Factory {
    specs: BTreeMap<Key, Spec>,
    claims: Arc<AtomicUsize>,
}
impl QueryFactory for Factory {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        self.claims.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Task {
            id,
            spec: self.specs.get(&key(id)).ok_or(errors().protocol)?.clone(),
        }))
    }
}
struct Task {
    id: QueryId,
    spec: Spec,
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.id
    }
    fn weight(&self) -> MemoryWeight {
        self.spec.weight
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        let mut pending = Vec::new();
        for dependency in &self.spec.dependencies {
            match cx.read(*dependency) {
                Poll::Pending => pending.push(*dependency),
                Poll::Ready(Err(error)) => return QueryPoll::Failed(error),
                Poll::Ready(Ok(_)) => (),
            }
        }
        if !pending.is_empty() {
            QueryPoll::Waiting(pending)
        } else if let Some(error) = self.spec.result {
            QueryPoll::Failed(error)
        } else {
            QueryPoll::Complete(product(self.id))
        }
    }
}
fn install(rt: &mut AdmittedRuntime, specs: &[(u64, Spec)]) -> Arc<AtomicUsize> {
    let claims = Arc::new(AtomicUsize::new(0));
    rt.register(
        1,
        Arc::new(Factory {
            specs: specs
                .iter()
                .map(|(n, spec)| (key(query(*n)), spec.clone()))
                .collect(),
            claims: claims.clone(),
        }),
    )
    .unwrap();
    claims
}
#[test]
fn single_worker_dependency_chain_releases_all_execution_slots() {
    let mut rt = runtime(1, 4 << 20);
    install(
        &mut rt,
        &[
            (0, Spec::deps(&[1])),
            (1, Spec::deps(&[2])),
            (2, Spec::deps(&[])),
        ],
    );
    let request = rt.request(&[query(0)]);
    assert_eq!(rt.wait(request), Ok(vec![product(query(0))]));
    assert_eq!(rt.stats().claims, 3);
    assert_eq!(rt.stats().edges, 2);
    assert_eq!(rt.stats().active, 0);
    assert!(rt.stats().waits >= 2);
}
#[test]
fn diamond_coalesces_with_multiple_requesters_and_preserves_root_order() {
    for jobs in [1, 2, 4, 8] {
        let mut rt = runtime(jobs, 32 << 20);
        let claims = install(
            &mut rt,
            &[
                (0, Spec::deps(&[1, 2])),
                (1, Spec::deps(&[3])),
                (2, Spec::deps(&[3])),
                (3, Spec::deps(&[])),
            ],
        );
        let first = rt.request(&[query(0), query(2)]);
        let second = rt.request(&[query(1), query(0)]);
        assert_eq!(
            rt.wait(first),
            Ok(vec![product(query(0)), product(query(2))])
        );
        assert_eq!(
            rt.wait(second),
            Ok(vec![product(query(1)), product(query(0))])
        );
        assert_eq!(claims.load(Ordering::SeqCst), 4);
        assert_eq!(rt.stats().edges, 4);
        assert_eq!(rt.poll(first), Poll::Ready(Err(errors().cancelled)));
        assert_eq!(rt.poll(second), Poll::Ready(Err(errors().cancelled)));
    }
}
#[test]
fn cyclic_member_tasks_refuse_and_component_queries_can_settle() {
    let mut rt = runtime(2, 8 << 20);
    install(
        &mut rt,
        &[
            (0, Spec::deps(&[1])),
            (1, Spec::deps(&[0])),
            (2, Spec::deps(&[])),
        ],
    );
    let request = rt.request(&[query(0), query(1), query(2)]);
    assert_eq!(rt.wait(request), Err(errors().cycle));
    assert_eq!(rt.stats().settled, 3);
}
#[test]
fn errors_still_execute_all_required_roots_and_select_first_root_failure() {
    let mut rt = runtime(4, 16 << 20);
    let mut first = Spec::deps(&[]);
    first.result = Some(reference(b"first"));
    let mut second = Spec::deps(&[]);
    second.result = Some(reference(b"second"));
    let claims = install(&mut rt, &[(0, first), (1, second), (2, Spec::deps(&[]))]);
    let request = rt.request(&[query(0), query(1), query(2)]);
    assert_eq!(rt.wait(request), Err(reference(b"first")));
    assert_eq!(claims.load(Ordering::SeqCst), 3);
    assert_eq!(rt.stats().settled, 3);
}
#[test]
fn zero_tiny_overflow_and_oversize_admission_never_raise_cap() {
    for budget in [0, 1, STACK as u64 - 1] {
        assert!(AdmittedRuntime::with_stack(Jobs::ONE, budget, errors(), STACK).is_err());
    }
    for weight in [
        MemoryWeight {
            scratch: 8 << 20,
            ..MemoryWeight::default()
        },
        MemoryWeight {
            scratch: u64::MAX,
            retained: 1,
            stack: 1,
        },
        MemoryWeight {
            stack: (STACK + 1) as u64,
            ..MemoryWeight::default()
        },
    ] {
        let mut rt = runtime(1, 4 << 20);
        let mut spec = Spec::deps(&[]);
        spec.weight = weight;
        install(&mut rt, &[(0, spec)]);
        let request = rt.request(&[query(0)]);
        assert_eq!(rt.wait(request), Err(errors().resource));
        assert!(rt.stats().peak_bytes <= 4 << 20);
    }
}
#[test]
fn idle_eviction_recreates_only_evicted_claims() {
    let mut rt = runtime(1, 4 << 20);
    let claims = install(&mut rt, &[(0, Spec::deps(&[]))]);
    let request = rt.request(&[query(0)]);
    rt.wait(request).unwrap();
    let request = rt.request(&[query(0)]);
    rt.wait(request).unwrap();
    assert_eq!(claims.load(Ordering::SeqCst), 1);
    rt.evict_to(0);
    assert_eq!(rt.stats().charged_bytes, STACK as u64);
    let request = rt.request(&[query(0)]);
    rt.wait(request).unwrap();
    assert_eq!(claims.load(Ordering::SeqCst), 2);
    rt.shutdown();
    assert_eq!(rt.stats().charged_bytes, 0);
    let request = rt.request(&[query(0)]);
    assert_eq!(rt.poll(request), Poll::Ready(Err(errors().cancelled)));
}
struct BlockingFactory {
    entered: mpsc::Sender<()>,
    release: Mutex<Option<mpsc::Receiver<()>>>,
}
impl QueryFactory for BlockingFactory {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        Ok(Box::new(BlockingTask {
            id,
            entered: self.entered.clone(),
            release: self.release.lock().unwrap().take(),
        }))
    }
}
struct BlockingTask {
    id: QueryId,
    entered: mpsc::Sender<()>,
    release: Option<mpsc::Receiver<()>>,
}
impl QueryTask for BlockingTask {
    fn id(&self) -> QueryId {
        self.id
    }
    fn weight(&self) -> MemoryWeight {
        MemoryWeight {
            scratch: 128,
            retained: 64,
            stack: 0,
        }
    }
    fn poll(&mut self, _: &mut dyn QueryContext) -> QueryPoll {
        if let Some(release) = self.release.take() {
            self.entered.send(()).unwrap();
            release.recv().unwrap();
        }
        QueryPoll::Complete(product(self.id))
    }
}
#[test]
fn cancelling_last_requester_drains_running_claim_and_can_request_again() {
    let mut rt = runtime(1, 4 << 20);
    let (entered, rx) = mpsc::channel();
    let (release, released) = mpsc::channel();
    rt.register(
        1,
        Arc::new(BlockingFactory {
            entered,
            release: Mutex::new(Some(released)),
        }),
    )
    .unwrap();
    let request = rt.request(&[query(0)]);
    rx.recv().unwrap();
    rt.cancel(request);
    let next = rt.request(&[query(0)]);
    release.send(()).unwrap();
    assert_eq!(rt.wait(next), Ok(vec![product(query(0))]));
    assert_eq!(rt.stats().claims, 2);
    assert_eq!(rt.stats().active, 0);
}
#[test]
fn cancelling_one_shared_request_preserves_the_live_claim() {
    let mut rt = runtime(1, 4 << 20);
    let (entered, rx) = mpsc::channel();
    let (release, released) = mpsc::channel();
    rt.register(
        1,
        Arc::new(BlockingFactory {
            entered,
            release: Mutex::new(Some(released)),
        }),
    )
    .unwrap();
    let first = rt.request(&[query(0)]);
    let second = rt.request(&[query(0)]);
    rx.recv().unwrap();
    rt.cancel(first);
    release.send(()).unwrap();
    assert_eq!(rt.wait(second), Ok(vec![product(query(0))]));
    assert_eq!(rt.stats().claims, 1);
}
#[test]
fn actual_edges_disjoint_costs_admission_and_completion_are_explicit() {
    let mut rt = runtime(1, 4 << 20);
    rt.record_events();
    install(&mut rt, &[(0, Spec::deps(&[1])), (1, Spec::deps(&[]))]);
    let request = rt.request(&[query(0)]);
    rt.wait(request).unwrap();
    let events = rt.take_events();
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, RuntimeEvent::Read { .. }))
            .count(),
        1
    );
    let mut live = 0;
    for event in events {
        match event {
            RuntimeEvent::Admit { charged, .. } => {
                live += 1;
                assert!(charged <= 4 << 20);
            }
            RuntimeEvent::Release { charged, .. } => {
                live -= 1;
                assert!(charged <= 4 << 20);
            }
            _ => (),
        }
        assert!((0..=1).contains(&live));
    }
    assert_eq!(live, 0);
}
#[test]
fn dispatch_is_frozen_and_unknown_kind_cannot_fabricate_a_product() {
    let mut rt = runtime(1, 4 << 20);
    let factory = Arc::new(Factory {
        specs: BTreeMap::new(),
        claims: Arc::new(AtomicUsize::new(0)),
    });
    rt.register(1, factory.clone()).unwrap();
    assert!(rt.register(1, factory.clone()).is_err());
    let request = rt.request(&[query(0)]);
    assert_eq!(rt.wait(request), Err(errors().protocol));
    assert!(rt.register(2, factory).is_err());
}

#[test]
fn simultaneous_revisions_and_observed_inputs_share_one_process_allowance() {
    let budget = ProcessBudget::new(16 << 20);
    let observed = budget.reserve(1 << 20).unwrap();
    let mut runtimes = Vec::new();
    for _ in 0..15 {
        let reservation = budget.reserve(1 << 20).unwrap();
        runtimes.push(
            AdmittedRuntime::with_reservation(Jobs::ONE, reservation, errors(), STACK).unwrap(),
        );
    }
    assert_eq!(budget.reserved(), 16 << 20);
    assert!(budget.reserve(1).is_err());
    runtimes.pop().unwrap().shutdown();
    assert_eq!(budget.reserved(), 15 << 20);
    let replacement = budget.reserve(1 << 20).unwrap();
    drop(replacement);
    drop(runtimes);
    assert_eq!(budget.reserved(), 1 << 20);
    drop(observed);
    assert_eq!(budget.reserved(), 0);
}
#[test]
fn measured_cost_prioritizes_large_ready_queries_and_releases_temporary_charge() {
    let mut rt = runtime(1, 4 << 20);
    install(
        &mut rt,
        &[
            (0, Spec::deps(&[])),
            (1, Spec::deps(&[])),
            (2, Spec::deps(&[])),
        ],
    );
    rt.set_cost(query(0), 10);
    rt.set_cost(query(1), 100);
    rt.set_cost(query(2), 1);
    rt.record_events();
    let request = rt.request(&[query(0), query(2), query(1)]);
    assert_eq!(
        rt.wait(request),
        Ok(vec![
            product(query(0)),
            product(query(2)),
            product(query(1))
        ])
    );
    let admitted: Vec<_> = rt
        .take_events()
        .into_iter()
        .filter_map(|e| match e {
            RuntimeEvent::Admit { query, .. } => Some(query),
            _ => None,
        })
        .collect();
    assert_eq!(admitted, [query(1), query(0), query(2)]);
    assert_eq!(rt.stats().charged_bytes, STACK as u64 + 3 * 16);
}
struct PanicFactory;
impl QueryFactory for PanicFactory {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        Ok(Box::new(PanicTask(id)))
    }
}
struct PanicTask(QueryId);
impl QueryTask for PanicTask {
    fn id(&self) -> QueryId {
        self.0
    }
    fn weight(&self) -> MemoryWeight {
        MemoryWeight {
            scratch: 128,
            retained: 16,
            stack: 0,
        }
    }
    fn poll(&mut self, _: &mut dyn QueryContext) -> QueryPoll {
        panic!("fixture panic")
    }
}
#[test]
fn panicking_task_releases_scratch_and_wakes_failed_dependents() {
    let mut rt = runtime(2, 8 << 20);
    install(
        &mut rt,
        &[(
            0,
            Spec {
                dependencies: vec![QueryId {
                    kind: 2,
                    address: query(1).address,
                }],
                ..Spec::deps(&[])
            },
        )],
    );
    rt.register(2, Arc::new(PanicFactory)).unwrap();
    let request = rt.request(&[query(0)]);
    assert_eq!(rt.wait(request), Err(errors().panicked));
    assert_eq!(rt.stats().active, 0);
    assert_eq!(rt.stats().charged_bytes, 2 * STACK as u64 + 2 * 16);
}
struct LyingFactory;
impl QueryFactory for LyingFactory {
    fn create(&self, q: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        Ok(Box::new(LyingTask(q)))
    }
}
struct LyingTask(QueryId);
impl QueryTask for LyingTask {
    fn id(&self) -> QueryId {
        self.0
    }
    fn weight(&self) -> MemoryWeight {
        MemoryWeight::default()
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if self.0 == query(0) {
            let _ = cx.read(query(1));
        }
        QueryPoll::Complete(product(self.0))
    }
}
#[test]
fn provisional_product_with_pending_reads_is_not_published() {
    let mut rt = runtime(1, 4 << 20);
    rt.register(1, Arc::new(LyingFactory)).unwrap();
    let request = rt.request(&[query(0)]);
    assert_eq!(rt.wait(request), Err(errors().protocol));
}
