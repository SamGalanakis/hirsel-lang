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
#[derive(Default)]
struct Events(Mutex<Vec<RuntimeEvent>>);
impl RuntimeObserver for Events {
    fn record(&self, event: RuntimeEvent) {
        self.0.lock().unwrap().push(event);
    }
}
fn trace(rt: &mut AdmittedRuntime) -> Arc<Events> {
    let events = Arc::new(Events::default());
    rt.observe(events.clone()).unwrap();
    events
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
    assert_eq!(
        rt.stats().charged_bytes - rt.stats().bookkeeping_bytes,
        STACK as u64
    );
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
    let events = trace(&mut rt);
    install(&mut rt, &[(0, Spec::deps(&[1])), (1, Spec::deps(&[]))]);
    let request = rt.request(&[query(0)]);
    rt.wait(request).unwrap();
    let events = std::mem::take(&mut *events.0.lock().unwrap());
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
    rt.set_cost(query(0), 10).unwrap();
    rt.set_cost(query(1), 100).unwrap();
    rt.set_cost(query(2), 1).unwrap();
    let events = trace(&mut rt);
    let request = rt.request(&[query(0), query(2), query(1)]);
    assert_eq!(
        rt.wait(request),
        Ok(vec![
            product(query(0)),
            product(query(2)),
            product(query(1))
        ])
    );
    let admitted: Vec<_> = std::mem::take(&mut *events.0.lock().unwrap())
        .into_iter()
        .filter_map(|e| match e {
            RuntimeEvent::Admit { query, .. } => Some(query),
            _ => None,
        })
        .collect();
    assert_eq!(admitted, [query(1), query(0), query(2)]);
    assert_eq!(
        rt.stats().charged_bytes - rt.stats().bookkeeping_bytes,
        STACK as u64 + 3 * 16
    );
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
    assert_eq!(
        rt.stats().charged_bytes - rt.stats().bookkeeping_bytes,
        2 * STACK as u64 + 2 * 16
    );
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

#[test]
fn metadata_exhaustion_refuses_requests_and_dependency_growth_without_leaking_slots() {
    let mut rt = runtime(1, STACK as u64 + 32 * ENTRY_BYTES);
    assert_eq!(rt.shared.lock().max_entries, 2);
    install(
        &mut rt,
        &[
            (0, Spec::deps(&[1])),
            (1, Spec::deps(&[2])),
            (2, Spec::deps(&[])),
        ],
    );
    let oversized = rt.request(&[query(0), query(1), query(2)]);
    assert_eq!(rt.poll(oversized), Poll::Ready(Err(errors().resource)));
    assert_eq!(rt.stats().claims, 0);
    let request = rt.request(&[query(0)]);
    assert_eq!(rt.wait(request), Err(errors().resource));
    assert_eq!(rt.stats().claims, 2);
    assert_eq!(rt.stats().active, 0);
    assert!(rt.stats().peak_bytes <= STACK as u64 + 32 * ENTRY_BYTES);
    for n in 10..30 {
        let request = rt.request(&[query(n)]);
        assert_eq!(rt.wait(request), Err(errors().protocol));
        assert!(rt.shared.lock().nodes.len() <= 2);
    }
}
struct PanickingFactory;
impl QueryFactory for PanickingFactory {
    fn create(&self, _: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        panic!("factory fixture")
    }
}
#[test]
fn failed_factory_construction_settles_without_poisoning_the_pool() {
    let mut rt = runtime(2, 8 << 20);
    rt.register(1, Arc::new(PanickingFactory)).unwrap();
    let request = rt.request(&[query(0), query(1)]);
    assert_eq!(rt.wait(request), Err(errors().panicked));
    rt.shutdown();
    assert_eq!(rt.stats().charged_bytes, 0);
}
#[test]
fn declared_stack_fits_reserved_worker_stack_without_being_charged_twice() {
    let mut rt = runtime(1, 2 * STACK as u64);
    let mut spec = Spec::deps(&[]);
    spec.weight.stack = STACK as u64;
    install(&mut rt, &[(0, spec)]);
    let request = rt.request(&[query(0)]);
    assert_eq!(rt.wait(request), Ok(vec![product(query(0))]));
    assert_eq!(rt.stats().workers, 1);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocalEvent {
    Create,
    Poll(usize),
    Drop,
}
struct AffineFactory {
    dependency: QueryId,
    events: mpsc::Sender<(QueryId, LocalEvent, std::thread::ThreadId)>,
    barrier: Option<Arc<std::sync::Barrier>>,
}
impl LocalQueryFactory for AffineFactory {
    fn weight(&self, _: QueryId) -> Result<MemoryWeight, Reference> {
        Ok(MemoryWeight {
            retained: 64,
            scratch: 32,
            stack: 0,
        })
    }
    fn create(&self, id: QueryId) -> Result<Box<dyn LocalQueryTask>, Reference> {
        let home = std::thread::current().id();
        self.events.send((id, LocalEvent::Create, home)).unwrap();
        Ok(Box::new(AffineTask {
            id,
            home,
            dependency: self.dependency,
            polls: std::rc::Rc::new(std::cell::Cell::new(0)),
            events: self.events.clone(),
            barrier: self.barrier.clone(),
        }))
    }
}
struct AffineTask {
    id: QueryId,
    home: std::thread::ThreadId,
    dependency: QueryId,
    // Deliberately !Send. Neither task state nor this Rc crosses a worker.
    polls: std::rc::Rc<std::cell::Cell<usize>>,
    events: mpsc::Sender<(QueryId, LocalEvent, std::thread::ThreadId)>,
    barrier: Option<Arc<std::sync::Barrier>>,
}
impl Drop for AffineTask {
    fn drop(&mut self) {
        assert_eq!(std::thread::current().id(), self.home);
        self.events
            .send((self.id, LocalEvent::Drop, self.home))
            .unwrap();
    }
}
impl LocalQueryTask for AffineTask {
    fn id(&self) -> QueryId {
        self.id
    }
    fn weight(&self) -> MemoryWeight {
        MemoryWeight {
            retained: 64,
            scratch: 32,
            stack: 0,
        }
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        assert_eq!(std::thread::current().id(), self.home);
        self.polls.set(self.polls.get() + 1);
        self.events
            .send((self.id, LocalEvent::Poll(self.polls.get()), self.home))
            .unwrap();
        if let Some(barrier) = self.barrier.take() {
            barrier.wait();
        }
        match cx.read(self.dependency) {
            Poll::Pending => QueryPoll::Waiting(vec![self.dependency]),
            Poll::Ready(Ok(_)) => QueryPoll::Complete(product(self.id)),
            Poll::Ready(Err(error)) => QueryPoll::Failed(error),
        }
    }
}
fn local_query(n: u64) -> QueryId {
    QueryId {
        kind: 2,
        address: query(n).address,
    }
}
#[test]
fn affine_wait_releases_single_worker_and_resumes_rc_state_without_replay() {
    let mut rt = runtime(1, 4 << 20);
    install(&mut rt, &[(0, Spec::deps(&[]))]);
    let (events, rx) = mpsc::channel();
    rt.register_local(
        2,
        Arc::new(AffineFactory {
            dependency: query(0),
            events,
            barrier: None,
        }),
    )
    .unwrap();
    let request = rt.request(&[local_query(1)]);
    assert_eq!(rt.wait(request), Ok(vec![product(local_query(1))]));
    let events: Vec<_> = rx.try_iter().collect();
    assert_eq!(
        events.iter().map(|e| e.1).collect::<Vec<_>>(),
        [
            LocalEvent::Create,
            LocalEvent::Poll(1),
            LocalEvent::Poll(2),
            LocalEvent::Drop
        ]
    );
    assert!(events.iter().all(|e| e.2 == events[0].2));
    assert_ne!(events[0].2, std::thread::current().id());
    assert_eq!(rt.stats().claims, 2);
    assert_eq!(rt.stats().active, 0);
}
#[test]
fn all_affine_workers_waiting_can_run_shared_send_dependency_and_keep_home_ownership() {
    let mut rt = runtime(2, 8 << 20);
    install(&mut rt, &[(0, Spec::deps(&[]))]);
    let (events, rx) = mpsc::channel();
    rt.register_local(
        2,
        Arc::new(AffineFactory {
            dependency: query(0),
            events,
            barrier: Some(Arc::new(std::sync::Barrier::new(2))),
        }),
    )
    .unwrap();
    let request = rt.request(&[local_query(1), local_query(2)]);
    assert_eq!(
        rt.wait(request),
        Ok(vec![product(local_query(1)), product(local_query(2))])
    );
    let events: Vec<_> = rx.try_iter().collect();
    let homes: Vec<_> = events
        .iter()
        .filter(|e| e.1 == LocalEvent::Create)
        .map(|e| e.2)
        .collect();
    assert_eq!(homes.len(), 2);
    assert_ne!(homes[0], homes[1]);
    for q in [local_query(1), local_query(2)] {
        let own: Vec<_> = events.iter().filter(|e| e.0 == q).collect();
        assert!(own.iter().all(|e| e.2 == own[0].2));
        assert_eq!(own.iter().filter(|e| e.1 == LocalEvent::Create).count(), 1);
        assert_eq!(own.iter().filter(|e| e.1 == LocalEvent::Drop).count(), 1);
    }
    assert_eq!(rt.stats().claims, 3);
    assert_eq!(rt.stats().workers, 2);
}
#[test]
fn affine_cancellation_and_shutdown_destroy_continuations_on_home_worker() {
    for shutdown in [false, true] {
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
        let (events, local_events) = mpsc::channel();
        rt.register_local(
            2,
            Arc::new(AffineFactory {
                dependency: query(0),
                events,
                barrier: None,
            }),
        )
        .unwrap();
        let request = rt.request(&[local_query(1)]);
        rx.recv().unwrap();
        // The affine parent is suspended, the same worker is running its Send
        // dependency, and cancellation must not run Drop on this host thread.
        if shutdown {
            let host = std::thread::spawn(move || {
                rt.shutdown();
                rt
            });
            release.send(()).unwrap();
            rt = host.join().unwrap();
        } else {
            rt.cancel(request);
            assert!(!local_events.try_iter().any(|e| e.1 == LocalEvent::Drop));
            release.send(()).unwrap();
            let mut state = rt.shared.lock();
            while !state.nodes.is_empty() || state.stats.active != 0 {
                state = rt.shared.changed.wait(state).unwrap();
            }
        }
        let events: Vec<_> = local_events.try_iter().collect();
        let dropped = events.iter().find(|e| e.1 == LocalEvent::Drop).unwrap();
        assert_ne!(dropped.2, std::thread::current().id());
        assert_eq!(rt.poll(request), Poll::Ready(Err(errors().cancelled)));
        rt.shutdown();
        assert_eq!(rt.stats().charged_bytes, 0);
    }
}
#[test]
fn affine_factory_queue_obeys_same_metadata_bound_before_creating_local_state() {
    let mut rt = runtime(1, STACK as u64 + 32 * ENTRY_BYTES);
    let (events, rx) = mpsc::channel();
    rt.register_local(
        2,
        Arc::new(AffineFactory {
            dependency: query(0),
            events,
            barrier: None,
        }),
    )
    .unwrap();
    let request = rt.request(&[local_query(0), local_query(1), local_query(2)]);
    assert_eq!(rt.poll(request), Poll::Ready(Err(errors().resource)));
    assert_eq!(rx.try_iter().count(), 0);
    assert_eq!(rt.stats().claims, 0);
}

struct Reads(mpsc::Sender<()>);
impl RuntimeObserver for Reads {
    fn record(&self, event: RuntimeEvent) {
        if let RuntimeEvent::Read { .. } = event {
            self.0.send(()).unwrap();
        }
    }
}
#[test]
fn new_dependent_keeps_its_latch_when_cancelled_inflight_claim_is_restarted() {
    let mut rt = runtime(2, 8 << 20);
    let (read, read_rx) = mpsc::channel();
    rt.observe(Arc::new(Reads(read))).unwrap();
    let (entered, entered_rx) = mpsc::channel();
    let (release, released) = mpsc::channel();
    rt.register(
        1,
        Arc::new(BlockingFactory {
            entered,
            release: Mutex::new(Some(released)),
        }),
    )
    .unwrap();
    let (events, local_rx) = mpsc::channel();
    rt.register_local(
        2,
        Arc::new(AffineFactory {
            dependency: query(0),
            events,
            barrier: None,
        }),
    )
    .unwrap();
    let old = rt.request(&[query(0)]);
    entered_rx.recv().unwrap();
    rt.cancel(old);
    // A fresh dependency attaches to the cancelled claim while its old poll is
    // still draining. The replacement must inherit that dependency's waiter.
    let request = rt.request(&[local_query(1)]);
    read_rx.recv().unwrap();
    release.send(()).unwrap();
    {
        let state = rt.shared.lock();
        let (_state, timeout) = rt
            .shared
            .changed
            .wait_timeout_while(state, std::time::Duration::from_secs(2), |s| {
                !s.request_complete(request.0)
            })
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "replacement lost a newly subscribed dependency latch"
        );
    }
    assert_eq!(
        rt.poll(request),
        Poll::Ready(Ok(vec![product(local_query(1))]))
    );
    assert_eq!(rt.stats().claims, 3);
    let events: Vec<_> = local_rx.try_iter().collect();
    assert_eq!(
        events.iter().filter(|e| e.1 == LocalEvent::Create).count(),
        1
    );
    assert_eq!(events.iter().filter(|e| e.1 == LocalEvent::Drop).count(), 1);
}

struct FutureFactory {
    steps: Arc<AtomicUsize>,
    invalid_wait: bool,
    reads: usize,
}
impl LocalQueryFactory for FutureFactory {
    fn weight(&self, _: QueryId) -> Result<MemoryWeight, Reference> {
        Ok(MemoryWeight {
            retained: 8192,
            scratch: 32,
            stack: 0,
        })
    }
    fn create(&self, id: QueryId) -> Result<Box<dyn LocalQueryTask>, Reference> {
        let steps = self.steps.clone();
        let invalid_wait = self.invalid_wait;
        let reads = self.reads;
        let state = std::rc::Rc::new(std::cell::Cell::new(0));
        LocalFutureTask::new(
            id,
            self.weight(id)?,
            reads,
            errors(),
            |dependencies| async move {
                steps.fetch_add(1, Ordering::SeqCst);
                if invalid_wait {
                    std::future::pending::<()>().await;
                }
                for dependency in [query(1), query(2)] {
                    dependencies.read(dependency).await?;
                    state.set(state.get() + 1);
                    steps.fetch_add(1, Ordering::SeqCst);
                }
                assert_eq!(state.get(), 2);
                // A repeated read reuses the task's resolved output, not a new
                // transformation or future invocation.
                dependencies.read(query(1)).await?;
                Ok(product(id))
            },
        )
        .map(|task| Box::new(task) as Box<dyn LocalQueryTask>)
    }
}
#[test]
fn local_future_keeps_rc_state_across_real_query_waits_without_restarting() {
    let mut rt = runtime(1, 4 << 20);
    install(&mut rt, &[(1, Spec::deps(&[])), (2, Spec::deps(&[]))]);
    let steps = Arc::new(AtomicUsize::new(0));
    rt.register_local(
        2,
        Arc::new(FutureFactory {
            steps: steps.clone(),
            invalid_wait: false,
            reads: 2,
        }),
    )
    .unwrap();
    let request = rt.request(&[local_query(0)]);
    assert_eq!(rt.wait(request), Ok(vec![product(local_query(0))]));
    assert_eq!(steps.load(Ordering::SeqCst), 3);
    assert_eq!(rt.stats().claims, 3);
    assert_eq!(rt.stats().edges, 2);
    assert_eq!(rt.stats().waits, 2);
}
#[test]
fn future_dependency_metadata_and_unregistered_suspensions_refuse_without_hanging() {
    for (invalid_wait, reads, expected) in
        [(true, 2, errors().protocol), (false, 1, errors().resource)]
    {
        let mut rt = runtime(1, 4 << 20);
        install(&mut rt, &[(1, Spec::deps(&[])), (2, Spec::deps(&[]))]);
        let steps = Arc::new(AtomicUsize::new(0));
        rt.register_local(
            2,
            Arc::new(FutureFactory {
                steps: steps.clone(),
                invalid_wait,
                reads,
            }),
        )
        .unwrap();
        let request = rt.request(&[local_query(0)]);
        assert_eq!(rt.wait(request), Err(expected));
        assert_eq!(
            steps.load(Ordering::SeqCst),
            if invalid_wait { 1 } else { 2 }
        );
        assert_eq!(rt.stats().active, 0);
    }
}

#[test]
fn host_completion_wait_does_not_hold_runtime_ownership_or_consume_results() {
    let mut rt = runtime(1, 4 << 20);
    let (entered, entered_rx) = mpsc::channel();
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
    entered_rx.recv().unwrap();
    let runtime = Arc::new(Mutex::new(rt));
    let completion = runtime.lock().unwrap().completion();
    let (finished, finished_rx) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        completion.wait(request);
        finished.send(()).unwrap();
    });
    // A host can still acquire the runtime and cancel while another host waits.
    runtime.lock().unwrap().cancel(request);
    finished_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("cancellation did not wake the unlocked host completion wait");
    waiter.join().unwrap();
    assert_eq!(
        runtime.lock().unwrap().poll(request),
        Poll::Ready(Err(errors().cancelled))
    );
    release.send(()).unwrap();
    runtime.lock().unwrap().shutdown();
}
#[test]
fn host_completion_handles_settlement_before_wait_and_runtime_shutdown() {
    let mut rt = runtime(1, 4 << 20);
    install(&mut rt, &[(0, Spec::deps(&[]))]);
    let completion = rt.completion();
    let request = rt.request(&[query(0)]);
    completion.wait(request);
    // Notification precedes polling; waiting twice does not consume completion.
    completion.wait(request);
    assert_eq!(rt.poll(request), Poll::Ready(Ok(vec![product(query(0))])));
    rt.shutdown();
    completion.wait(request);
    let refused = rt.request(&[query(1)]);
    completion.wait(refused);
    assert_eq!(rt.poll(refused), Poll::Ready(Err(errors().cancelled)));
}

struct DroppingFactory {
    gates: Mutex<Option<DropGates>>,
}
struct DropGates {
    entered: mpsc::Sender<()>,
    release_poll: mpsc::Receiver<()>,
    dropping: mpsc::Sender<()>,
    release_drop: mpsc::Receiver<()>,
}
struct DroppingTask {
    id: QueryId,
    gates: Option<DropGates>,
}
impl LocalQueryFactory for DroppingFactory {
    fn weight(&self, _: QueryId) -> Result<MemoryWeight, Reference> {
        Ok(MemoryWeight {
            retained: 128,
            scratch: 32,
            stack: 0,
        })
    }
    fn create(&self, id: QueryId) -> Result<Box<dyn LocalQueryTask>, Reference> {
        Ok(Box::new(DroppingTask {
            id,
            gates: self.gates.lock().unwrap().take(),
        }))
    }
}
impl LocalQueryTask for DroppingTask {
    fn id(&self) -> QueryId {
        self.id
    }
    fn weight(&self) -> MemoryWeight {
        MemoryWeight {
            retained: 128,
            scratch: 32,
            stack: 0,
        }
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if let Some(gates) = &self.gates {
            assert!(cx.read(query(1)).is_pending());
            gates.entered.send(()).unwrap();
            gates.release_poll.recv().unwrap();
            return QueryPoll::Waiting(vec![query(1)]);
        }
        QueryPoll::Complete(product(self.id))
    }
}
impl Drop for DroppingTask {
    fn drop(&mut self) {
        if let Some(gates) = &self.gates {
            gates.dropping.send(()).unwrap();
            gates.release_drop.recv().unwrap();
        }
    }
}
#[test]
fn new_request_during_cancelled_local_destruction_keeps_the_replacement_claim() {
    let mut rt = runtime(1, 4 << 20);
    install(&mut rt, &[(1, Spec::deps(&[]))]);
    let (entered, entered_rx) = mpsc::channel();
    let (release_poll, released_poll) = mpsc::channel();
    let (dropping, dropping_rx) = mpsc::channel();
    let (release_drop, released_drop) = mpsc::channel();
    rt.register_local(
        2,
        Arc::new(DroppingFactory {
            gates: Mutex::new(Some(DropGates {
                entered,
                release_poll: released_poll,
                dropping,
                release_drop: released_drop,
            })),
        }),
    )
    .unwrap();
    let old = rt.request(&[local_query(0)]);
    entered_rx.recv().unwrap();
    rt.cancel(old);
    release_poll.send(()).unwrap();
    // The old bounded poll has returned and its continuation is being destroyed
    // outside the scheduler lock. Reattach before that home handoff completes.
    dropping_rx.recv().unwrap();
    let request = rt.request(&[local_query(0)]);
    release_drop.send(()).unwrap();
    {
        let state = rt.shared.lock();
        let (_state, timeout) = rt
            .shared
            .changed
            .wait_timeout_while(state, std::time::Duration::from_secs(2), |s| {
                !s.request_complete(request.0)
            })
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "new live request was lost during old continuation destruction"
        );
    }
    assert_eq!(
        rt.poll(request),
        Poll::Ready(Ok(vec![product(local_query(0))]))
    );
    assert_eq!(rt.stats().claims, 3);
}

struct FrameFactory {
    owner: Arc<MemoryReservation>,
    drops: Arc<AtomicUsize>,
}
struct FrameTask {
    id: QueryId,
    next: u64,
    allocation: Vec<u8>,
    _owner: Arc<MemoryReservation>,
    drops: Arc<AtomicUsize>,
}
impl QueryFactory for FrameFactory {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        Ok(Box::new(FrameTask {
            id,
            next: 1,
            allocation: vec![],
            _owner: self.owner.clone(),
            drops: self.drops.clone(),
        }))
    }
}
impl Drop for FrameTask {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
impl QueryTask for FrameTask {
    fn id(&self) -> QueryId {
        self.id
    }
    fn weight(&self) -> MemoryWeight {
        MemoryWeight {
            scratch: 64 << 10,
            retained: (1 << 20) + 16,
            stack: 0,
        }
    }
    fn continuation_bytes(&self) -> u64 {
        1 << 20
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if self.allocation.is_empty() {
            self.allocation = vec![0; 1 << 20];
        }
        if self.id == query(0) {
            while self.next <= 12 {
                match cx.read(query(self.next)) {
                    Poll::Pending => return QueryPoll::Waiting(vec![query(self.next)]),
                    Poll::Ready(Err(error)) => return QueryPoll::Failed(error),
                    Poll::Ready(Ok(_)) => self.next += 1,
                }
            }
        }
        QueryPoll::Complete(product(self.id))
    }
}
#[test]
fn continuation_frames_retire_after_drop_without_retiring_shared_output_owners() {
    let process = ProcessBudget::new(5 << 20);
    let owner = Arc::new(process.reserve(1 << 20).unwrap());
    let drops = Arc::new(AtomicUsize::new(0));
    let factory = Arc::new(FrameFactory {
        owner: owner.clone(),
        drops: drops.clone(),
    });
    let mut rt = AdmittedRuntime::with_reservation(
        Jobs::ONE,
        process.reserve(4 << 20).unwrap(),
        errors(),
        STACK,
    )
    .unwrap();
    rt.register(1, factory.clone()).unwrap();
    let request = rt.request(&[query(0)]);
    assert_eq!(rt.wait(request), Ok(vec![product(query(0))]));
    assert_eq!(drops.load(Ordering::SeqCst), 13);
    assert_eq!(
        rt.stats().charged_bytes - rt.stats().bookkeeping_bytes,
        STACK as u64 + 13 * 16
    );
    assert!(rt.stats().peak_bytes <= 4 << 20);
    let repeated = rt.request(&[query(0)]);
    assert_eq!(rt.wait(repeated), Ok(vec![product(query(0))]));
    assert_eq!(
        drops.load(Ordering::SeqCst),
        13,
        "settled compact outcomes remain reusable"
    );
    drop(rt);
    assert_eq!(
        process.reserved(),
        1 << 20,
        "the separately owned products outlive continuations and workers"
    );
    drop(factory);
    drop(owner);
    assert_eq!(process.reserved(), 0);
}

struct LocalFrameFactory {
    owner: Arc<MemoryReservation>,
    drops: Arc<AtomicUsize>,
}
struct LocalFrame {
    id: QueryId,
    allocation: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
    _owner: Arc<MemoryReservation>,
    drops: Arc<AtomicUsize>,
    home: std::thread::ThreadId,
}
impl LocalQueryFactory for LocalFrameFactory {
    fn weight(&self, _: QueryId) -> Result<MemoryWeight, Reference> {
        Ok(MemoryWeight {
            scratch: 64 << 10,
            retained: (1 << 20) + 16,
            stack: 0,
        })
    }
    fn create(&self, id: QueryId) -> Result<Box<dyn LocalQueryTask>, Reference> {
        Ok(Box::new(LocalFrame {
            id,
            allocation: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
            _owner: self.owner.clone(),
            drops: self.drops.clone(),
            home: std::thread::current().id(),
        }))
    }
}
impl Drop for LocalFrame {
    fn drop(&mut self) {
        assert_eq!(self.home, std::thread::current().id());
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
impl LocalQueryTask for LocalFrame {
    fn id(&self) -> QueryId {
        self.id
    }
    fn weight(&self) -> MemoryWeight {
        MemoryWeight {
            scratch: 64 << 10,
            retained: (1 << 20) + 16,
            stack: 0,
        }
    }
    fn continuation_bytes(&self) -> u64 {
        1 << 20
    }
    fn poll(&mut self, _: &mut dyn QueryContext) -> QueryPoll {
        *self.allocation.borrow_mut() = vec![0; 1 << 20];
        QueryPoll::Complete(product(self.id))
    }
}
#[test]
fn local_continuation_frame_charge_retires_only_after_home_destruction() {
    let process = ProcessBudget::new(5 << 20);
    let owner = Arc::new(process.reserve(1 << 20).unwrap());
    let drops = Arc::new(AtomicUsize::new(0));
    let factory = Arc::new(LocalFrameFactory {
        owner: owner.clone(),
        drops: drops.clone(),
    });
    let mut rt = AdmittedRuntime::with_reservation(
        Jobs::ONE,
        process.reserve(4 << 20).unwrap(),
        errors(),
        STACK,
    )
    .unwrap();
    rt.register_local(1, factory.clone()).unwrap();
    let request = rt.request(&[query(0)]);
    assert_eq!(rt.wait(request), Ok(vec![product(query(0))]));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(
        rt.stats().charged_bytes - rt.stats().bookkeeping_bytes,
        STACK as u64 + 16
    );
    drop(rt);
    assert_eq!(process.reserved(), 1 << 20);
    drop(factory);
    drop(owner);
    assert_eq!(process.reserved(), 0);
}

struct FrameWaiting(mpsc::Sender<()>);
impl RuntimeObserver for FrameWaiting {
    fn record(&self, event: RuntimeEvent) {
        if matches!(event,RuntimeEvent::Waiting(id) if id.kind==2) {
            let _ = self.0.send(());
        }
    }
}
struct CancelFrameFactory {
    owner: Arc<MemoryReservation>,
    dropping: mpsc::Sender<(std::thread::ThreadId, std::thread::ThreadId)>,
    release: Mutex<Option<mpsc::Receiver<()>>>,
}
struct CancelFrame {
    id: QueryId,
    allocation: std::rc::Rc<Vec<u8>>,
    _owner: Arc<MemoryReservation>,
    home: std::thread::ThreadId,
    dropping: mpsc::Sender<(std::thread::ThreadId, std::thread::ThreadId)>,
    release: mpsc::Receiver<()>,
}
impl LocalQueryFactory for CancelFrameFactory {
    fn weight(&self, _: QueryId) -> Result<MemoryWeight, Reference> {
        Ok(MemoryWeight {
            scratch: 64 << 10,
            retained: (1 << 20) + 16,
            stack: 0,
        })
    }
    fn create(&self, id: QueryId) -> Result<Box<dyn LocalQueryTask>, Reference> {
        Ok(Box::new(CancelFrame {
            id,
            allocation: std::rc::Rc::new(Vec::new()),
            _owner: self.owner.clone(),
            home: std::thread::current().id(),
            dropping: self.dropping.clone(),
            release: self.release.lock().unwrap().take().unwrap(),
        }))
    }
}
impl LocalQueryTask for CancelFrame {
    fn id(&self) -> QueryId {
        self.id
    }
    fn weight(&self) -> MemoryWeight {
        MemoryWeight {
            scratch: 64 << 10,
            retained: (1 << 20) + 16,
            stack: 0,
        }
    }
    fn continuation_bytes(&self) -> u64 {
        1 << 20
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        self.allocation = std::rc::Rc::new(vec![0; 1 << 20]);
        match cx.read(query(1)) {
            Poll::Pending => QueryPoll::Waiting(vec![query(1)]),
            Poll::Ready(Err(error)) => QueryPoll::Failed(error),
            Poll::Ready(Ok(_)) => QueryPoll::Complete(product(self.id)),
        }
    }
}
impl Drop for CancelFrame {
    fn drop(&mut self) {
        self.dropping
            .send((self.home, std::thread::current().id()))
            .unwrap();
        self.release.recv().unwrap();
    }
}
#[test]
fn local_continuation_frame_cancellation_keeps_charge_until_home_drop_finishes() {
    let process = ProcessBudget::new(9 << 20);
    let owner = Arc::new(process.reserve(1 << 20).unwrap());
    let (dropping, drop_started) = mpsc::channel();
    let (release_drop, release) = mpsc::channel();
    let factory = Arc::new(CancelFrameFactory {
        owner: owner.clone(),
        dropping,
        release: Mutex::new(Some(release)),
    });
    let (entered, child_entered) = mpsc::channel();
    let (release_child, release) = mpsc::channel();
    let child = Arc::new(BlockingFactory {
        entered,
        release: Mutex::new(Some(release)),
    });
    let mut rt = AdmittedRuntime::with_reservation(
        Jobs::new(2),
        process.reserve(8 << 20).unwrap(),
        errors(),
        STACK,
    )
    .unwrap();
    rt.register(1, child.clone()).unwrap();
    rt.register_local(2, factory.clone()).unwrap();
    let (waiting, suspended) = mpsc::channel();
    rt.observe(Arc::new(FrameWaiting(waiting))).unwrap();
    let local = QueryId {
        kind: 2,
        ..query(0)
    };
    // The child occupies one worker before the frame exists, so the frame's
    // home is the other, idle worker and can run its cancellation cleanup.
    let held = rt.request(&[query(1)]);
    let started = child_entered.recv();
    let request = rt.request(&[local]);
    let waited = suspended.recv();
    rt.cancel(request);
    let dropped = drop_started.recv();
    let during = rt.stats();
    // Release the held destructors before any assertion can unwind the test.
    let _ = release_drop.send(());
    let _ = release_child.send(());
    rt.cancel(held);
    rt.shutdown();
    assert!(started.is_ok(), "child occupied one worker first");
    assert!(
        waited.is_ok(),
        "local continuation suspended before cancellation"
    );
    let (home, actual) = dropped.expect("home destruction began");
    assert_eq!(home, actual);
    assert!(
        during.charged_bytes - during.bookkeeping_bytes >= 2 * STACK as u64 + (1 << 20),
        "frame remains charged while its owner is still being destroyed"
    );
    assert!(during.peak_bytes <= 8 << 20);
    drop(rt);
    assert_eq!(
        process.reserved(),
        1 << 20,
        "separately retained output owner survives cancelled frame cleanup"
    );
    drop(factory);
    drop(child);
    drop(owner);
    assert_eq!(process.reserved(), 0);
}
#[test]
fn explicit_query_inventory_uses_the_same_grant_beyond_the_default_partition() {
    let bytes = STACK as u64 + 32 * ENTRY_BYTES;
    let budget = ProcessBudget::new(bytes);
    let reservation = budget.reserve(bytes).unwrap();
    let mut rt =
        AdmittedRuntime::with_metadata(Jobs::ONE, reservation, errors(), STACK, Some(3)).unwrap();
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
    assert_eq!(
        rt.stats().bookkeeping_bytes,
        AdmittedRuntime::query_capacity_bytes(3).unwrap()
    );
    assert!(rt.stats().peak_bytes <= bytes);
    assert_eq!(budget.reserved(), bytes);
    rt.shutdown();
    assert_eq!(budget.reserved(), 0);
}
#[test]
fn explicit_query_inventory_refuses_empty_overflow_and_insufficient_grants() {
    let bytes = STACK as u64 + 32 * ENTRY_BYTES;
    let budget = ProcessBudget::new(bytes);
    for entries in [0, 33, usize::MAX] {
        assert!(
            AdmittedRuntime::with_metadata(
                Jobs::ONE,
                budget.reserve(bytes).unwrap(),
                errors(),
                STACK,
                Some(entries)
            )
            .is_err()
        );
        assert_eq!(budget.reserved(), 0);
    }
    assert_eq!(
        AdmittedRuntime::query_capacity_bytes(3),
        Some(3 * ENTRY_BYTES)
    );
}

#[test]
fn public_default_capacity_matches_effective_worker_stack_admission() {
    for jobs in [Jobs::ONE, Jobs::new(16)] {
        for bytes in [
            COMPILER_STACK_BYTES as u64 - 1,
            COMPILER_STACK_BYTES as u64,
            512 << 20,
        ] {
            let predicted = AdmittedRuntime::default_query_capacity(jobs, bytes);
            let actual = AdmittedRuntime::new(
                jobs,
                ProcessBudget::new(bytes).reserve(bytes).unwrap(),
                errors(),
            );
            match (predicted, actual) {
                (None, Err(_)) => (),
                (Some(predicted), Ok(mut runtime)) => {
                    assert_eq!(predicted, runtime.shared.lock().max_entries);
                    assert!(
                        AdmittedRuntime::query_capacity_bytes(predicted).unwrap()
                            <= runtime.stats().bookkeeping_bytes
                    );
                    runtime.shutdown();
                }
                _ => panic!("default metadata planning and construction disagree"),
            }
        }
    }
}

#[test]
fn shutdown_returns_one_charge_after_cleanup_and_reuses_it_across_admitted_rounds() {
    for jobs in [1, 2] {
        let budget = ProcessBudget::new(8 << 20);
        let mut parent = budget.reserve(8 << 20).unwrap();
        for round in 0..3 {
            let grant = parent.split(6 << 20).unwrap();
            let mut rt =
                AdmittedRuntime::with_reservation(Jobs::new(jobs), grant, errors(), STACK).unwrap();
            let claims = install(&mut rt, &[(0, Spec::deps(&[1])), (1, Spec::deps(&[]))]);
            let factory_owner = Arc::downgrade(&claims);
            let request = rt.request(&[query(0)]);
            assert_eq!(rt.wait(request), Ok(vec![product(query(0))]));
            assert_eq!(claims.load(Ordering::SeqCst), 2);
            drop(claims);
            let grant = rt
                .shutdown_with_reservation()
                .expect("one live runtime charge");
            assert_eq!(grant.bytes(), 6 << 20);
            assert_eq!(rt.stats().active, 0);
            assert_eq!(rt.stats().bookkeeping_bytes, 0);
            assert_eq!(rt.stats().charged_bytes, 0);
            assert!(
                factory_owner.upgrade().is_none(),
                "round {round}: factories retire before grant return"
            );
            assert!(rt.shutdown_with_reservation().is_none());
            assert_eq!(budget.reserved(), 8 << 20);
            assert!(
                budget.reserve(1).is_err(),
                "returned reservation stays charged"
            );
            assert!(parent.join(grant).is_ok());
            drop(rt);
            assert_eq!(parent.bytes(), 8 << 20);
            assert_eq!(
                budget.reserved(),
                8 << 20,
                "old runtime Drop cannot release a reused charge"
            );
        }
        drop(parent);
        assert_eq!(budget.reserved(), 0);
    }
}
