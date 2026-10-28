//! One claim and completion latch per query, adapted from rustc 1.90
//! query/job.rs:183-269. Waiters here are continuations, never sleeping workers.
//! Ready ordering follows Go ebe2feef compile.go:139-184 (longest first).
//! Worker-owned task state follows tsgo 89d5d5b checkerpool.go:305,469.
//! Pending-edge counts follow Go 33e2e22 work/exec.go:126-135,189-210.
//! Here subscriptions are installed on reads and survive cancelled claims.
//! Persistent idle stacks remain charged; unlike the former native semaphore,
//! an oversized singleton cannot exceed the budget. S0 owns semantic validity.
use super::*;
use hirsel_intern::jobs::{COMPILER_STACK_BYTES, Jobs};
use hirsel_syntax::ContentHash;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Instant;

type Key = (u32, ContentHash);
type Outcome = Result<ProductRef, Reference>;
const REFUSED_REQUEST: u64 = 1 << 63;
// Conservative space for a node plus its tree/queue indexes, request roots,
// costs, four edges/subscriptions and temporary traversal work. These bounds
// supplement producer payload weights, whose RSS must still be measured.
const ENTRY_BYTES: u64 = 4096;

/// The engine installs a streaming observer, with separately budgeted buffers.
/// Called under the scheduler lock: do not reenter the runtime or block on work.
/// Production tracing never retains an unbounded in-runtime event history.
pub trait RuntimeObserver: Send + Sync {
    fn record(&self, event: RuntimeEvent);
}
fn key(id: QueryId) -> Key {
    (id.kind, id.address.0)
}
fn id(key: Key) -> QueryId {
    QueryId {
        kind: key.0,
        address: Address(key.1),
    }
}

/// Engine-owned immutable diagnostic records, never synthesized as successes.
#[derive(Clone, Copy)]
pub struct RuntimeErrors {
    pub resource: Reference,
    pub cycle: Reference,
    pub cancelled: Reference,
    pub protocol: Reference,
    pub panicked: Reference,
}

/// Disjoint poll costs and actual edges; wall intervals are not dependency guesses.
#[derive(Clone, Debug)]
pub enum RuntimeEvent {
    Claim(QueryId),
    Read {
        query: QueryId,
        dependency: QueryId,
    },
    Admit {
        query: QueryId,
        worker: usize,
        elapsed_ns: u64,
        charged: u64,
    },
    Release {
        query: QueryId,
        worker: usize,
        elapsed_ns: u64,
        cpu_ns: u64,
        charged: u64,
    },
    Waiting(QueryId),
    Wake(QueryId),
    Settled {
        query: QueryId,
        result: Outcome,
    },
    Request {
        request: RequestId,
        roots: Vec<QueryId>,
    },
    Completed(RequestId),
    Cancelled(RequestId),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeStats {
    pub claims: u64,
    pub polls: u64,
    pub waits: u64,
    pub wakes: u64,
    pub edges: u64,
    pub peak_bytes: u64,
    pub charged_bytes: u64,
    pub active: usize,
    pub settled: u64,
    pub bookkeeping_bytes: u64,
    pub workers: usize,
}

enum Status {
    Ready,
    Running,
    Waiting,
    Cancelling,
    Settled(Outcome),
}
#[derive(Clone)]
enum Factory {
    Send(Arc<dyn QueryFactory>),
    Local(Arc<dyn LocalQueryFactory>),
}
enum Task {
    Send(Option<Box<dyn QueryTask>>),
    Local {
        factory: Arc<dyn LocalQueryFactory>,
        home: Option<usize>,
    },
}
struct Node {
    task: Task,
    weight: MemoryWeight,
    continuation: u64,
    status: Status,
    dependencies: BTreeSet<Key>,
    // One count per reverse subscription, including reads made during a poll.
    // Completion can race that poll's return to Waiting under the same lock.
    unsatisfied: usize,
    waiters: BTreeSet<Key>,
    live: bool,
    // A cancelled in-flight poll is drained before a new claim can replace it.
    discard: bool,
}
struct State {
    factories: BTreeMap<u32, Factory>,
    nodes: BTreeMap<Key, Node>,
    ready: BTreeSet<(Reverse<u64>, Key)>,
    cleanup: Vec<BTreeMap<Key, Option<Reference>>>,
    costs: BTreeMap<Key, u64>,
    requests: BTreeMap<u64, Vec<Key>>,
    next_request: u64,
    retained: u64,
    scratch: u64,
    stacks: u64,
    budget: u64,
    metadata: u64,
    edges: usize,
    max_entries: usize,
    errors: RuntimeErrors,
    stopped: bool,
    started: bool,
    stats: RuntimeStats,
    observer: Option<Arc<dyn RuntimeObserver>>,
    origin: Instant,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    caller: Mutex<()>,
}

/// Runtime for one immutable engine input snapshot. Factories validate S0 reads
/// before returning a hit. Revisions are selected by the engine, not RequestId.
/// Poll is nonblocking; wait is an optional host completion wait, never a query
/// dependency wait. Every query poll must return at a bounded cancellation point.
/// Producers own ordered registration/fuel/diagnostic commit tasks; speculative
/// task completion does not authorize publication outside that ledger.
pub struct AdmittedRuntime {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
    reservation: Option<MemoryReservation>,
    caller_stack: Option<u64>,
}

/// A host completion wait independent of the runtime's outer ownership lock.
/// Clone while holding that lock, then release it before waiting. This handle
/// does not consume a result; cancellation and shutdown wake it too. A compiler
/// runtime without workers drives its admitted graph on a compiler stack here.
#[derive(Clone)]
pub struct RuntimeCompletion {
    shared: Arc<Shared>,
    caller_stack: Option<u64>,
}
impl RuntimeCompletion {
    pub fn wait(&self, request: RequestId) {
        if let Some(stack) = self.caller_stack {
            let _caller = self.shared.caller.lock().unwrap_or_else(|e| e.into_inner());
            if !self.shared.lock().request_complete(request.0) {
                crate::with_compiler_stack(|| run_worker(self.shared.clone(), 0, stack, true));
            }
        }
        let mut state = self.shared.lock();
        while !state.request_complete(request.0) && !state.stopped {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}

// Same policy as Jobs::worker_count_with_cap, also used with smaller
// test stacks. Both public capacity planning and construction share it.
fn runtime_layout(jobs: Jobs, budget: u64, stack: usize) -> Option<(usize, u64)> {
    let stack = u64::try_from(stack).ok()?;
    if stack == 0 || budget < stack {
        return None;
    }
    let count = jobs
        .get()
        .min((budget / stack.checked_mul(2)?).max(1) as usize);
    Some((count, budget - count as u64 * stack))
}

impl AdmittedRuntime {
    pub fn new(
        jobs: Jobs,
        reservation: MemoryReservation,
        errors: RuntimeErrors,
    ) -> std::io::Result<Self> {
        Self::with_reservation(jobs, reservation, errors, COMPILER_STACK_BYTES)
    }
    /// Reserve scheduler rows for the immutable query inventory within the
    /// supplied runtime grant. Worker stacks and task working memory use its
    /// remainder; this never obtains an additional process reservation.
    /// A one-worker compiler runs on its caller. If no additional worker can
    /// start, completion waits drive the same admitted graph on the caller.
    pub fn with_query_capacity(
        jobs: Jobs,
        reservation: MemoryReservation,
        errors: RuntimeErrors,
        entries: usize,
    ) -> std::io::Result<Self> {
        Self::with_metadata(
            jobs,
            reservation,
            errors,
            COMPILER_STACK_BYTES,
            Some(entries),
            true,
        )
    }
    /// Conservative allocation bound shared by admission planning and the
    /// constructor. Includes node indexes, four edges, roots and traversal.
    pub fn query_capacity_bytes(entries: usize) -> Option<u64> {
        u64::try_from(entries).ok()?.checked_mul(ENTRY_BYTES)
    }
    /// Bytes left after the worker stacks this reservation actually admits.
    /// Planners subtract query bookkeeping from this same layout before
    /// assigning producer scratch and continuation grants.
    pub fn working_bytes(jobs: Jobs, runtime_bytes: u64) -> Option<u64> {
        runtime_layout(jobs, runtime_bytes, COMPILER_STACK_BYTES).map(|(_, bytes)| bytes)
    }
    /// Original constructor capacity, including its effective worker-stack
    /// selection. Hosts can preserve subscriber/factory headroom when a small
    /// query inventory would otherwise reserve fewer rows than `new`.
    pub fn default_query_capacity(jobs: Jobs, runtime_bytes: u64) -> Option<usize> {
        let working = Self::working_bytes(jobs, runtime_bytes)?;
        Some(((working / 16) / ENTRY_BYTES).min(usize::MAX as u64) as usize)
    }
    #[cfg(test)]
    fn with_stack(
        jobs: Jobs,
        budget: u64,
        errors: RuntimeErrors,
        stack: usize,
    ) -> std::io::Result<Self> {
        Self::with_reservation(
            jobs,
            ProcessBudget::new(budget).reserve(budget)?,
            errors,
            stack,
        )
    }
    /// Construct a runtime with an explicit physical worker-stack reservation.
    /// Callers must provide the stack required by their registered phases;
    /// ordinary compiler callers use `new` and its compiler-stack default.
    pub fn with_reservation(
        jobs: Jobs,
        reservation: MemoryReservation,
        errors: RuntimeErrors,
        stack: usize,
    ) -> std::io::Result<Self> {
        Self::with_metadata(jobs, reservation, errors, stack, None, false)
    }
    fn with_metadata(
        jobs: Jobs,
        reservation: MemoryReservation,
        errors: RuntimeErrors,
        stack: usize,
        entries: Option<usize>,
        compiler: bool,
    ) -> std::io::Result<Self> {
        let budget = reservation.bytes();
        let (count, working) = runtime_layout(jobs, budget, stack).ok_or_else(|| {
            std::io::Error::other("query budget cannot reserve one compiler stack")
        })?;
        let metadata = match entries {
            None => working / 16,
            Some(entries) => Self::query_capacity_bytes(entries)
                .filter(|bytes| entries != 0 && *bytes <= working)
                .ok_or_else(|| std::io::Error::other("query inventory exceeds runtime grant"))?,
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                factories: BTreeMap::new(),
                nodes: BTreeMap::new(),
                ready: BTreeSet::new(),
                cleanup: (0..count).map(|_| BTreeMap::new()).collect(),
                costs: BTreeMap::new(),
                requests: BTreeMap::new(),
                next_request: 0,
                retained: 0,
                scratch: 0,
                stacks: count as u64 * stack as u64,
                budget,
                metadata,
                edges: 0,
                max_entries: (metadata / ENTRY_BYTES).min(usize::MAX as u64) as usize,
                errors,
                stopped: false,
                started: false,
                stats: RuntimeStats::default(),
                observer: None,
                origin: Instant::now(),
            }),
            changed: Condvar::new(),
            caller: Mutex::new(()),
        });
        let mut runtime = Self {
            shared,
            workers: Vec::new(),
            reservation: Some(reservation),
            caller_stack: (compiler && jobs == Jobs::ONE).then_some(stack as u64),
        };
        for worker in 0..count {
            if runtime.caller_stack.is_some() {
                break;
            }
            let shared = runtime.shared.clone();
            match std::thread::Builder::new()
                .name("hirsel-query".into())
                .stack_size(stack)
                .spawn(move || run_worker(shared, worker, stack as u64, false))
            {
                Ok(handle) => runtime.workers.push(handle),
                Err(error) => {
                    if compiler {
                        hirsel_intern::jobs::warn_spawn_failure(
                            &error,
                            runtime.workers.len().max(1),
                        );
                        if runtime.workers.is_empty() {
                            runtime.caller_stack = Some(stack as u64);
                        }
                    }
                    let mut state = runtime.shared.lock();
                    state.stacks = runtime
                        .workers
                        .len()
                        .max(usize::from(runtime.caller_stack.is_some()))
                        as u64
                        * stack as u64;
                    if runtime.workers.is_empty() && runtime.caller_stack.is_none() {
                        return Err(error);
                    }
                    break;
                }
            }
        }
        {
            let mut state = runtime.shared.lock();
            state.stats.workers = runtime
                .workers
                .len()
                .max(usize::from(runtime.caller_stack.is_some()));
            state.stacks = state.stats.workers as u64 * stack as u64;
            state.account();
        }
        Ok(runtime)
    }
    /// Register each producer kind exactly once before any request starts.
    pub fn register(
        &mut self,
        kind: u32,
        factory: Arc<dyn QueryFactory>,
    ) -> Result<(), &'static str> {
        self.register_factory(kind, Factory::Send(factory))
    }
    pub fn register_local(
        &mut self,
        kind: u32,
        factory: Arc<dyn LocalQueryFactory>,
    ) -> Result<(), &'static str> {
        self.register_factory(kind, Factory::Local(factory))
    }
    fn register_factory(&mut self, kind: u32, factory: Factory) -> Result<(), &'static str> {
        let mut state = self.shared.lock();
        if state.started {
            return Err("query dispatch is frozen after first request");
        }
        if state.factories.len() >= state.max_entries.max(1) {
            return Err("query factory allowance exhausted");
        }
        if state.factories.contains_key(&kind) {
            return Err("query kind already registered");
        }
        state.factories.insert(kind, factory);
        Ok(())
    }
    /// Measured disjoint CPU cost from a prior comparable query, used only for
    /// ready priority. Neither identity nor semantic accounting depends on it.
    pub fn set_cost(&mut self, query: QueryId, cpu_ns: u64) -> Result<(), &'static str> {
        let mut state = self.shared.lock();
        let query = key(query);
        if !state.costs.contains_key(&query) && state.costs.len() >= state.max_entries {
            return Err("query cost allowance exhausted");
        }
        state.unqueue(query);
        state.costs.insert(query, cpu_ns.max(1));
        if state
            .nodes
            .get(&query)
            .is_some_and(|n| matches!(n.status, Status::Ready))
        {
            state.queue(query);
        }
        Ok(())
    }
    /// Observer ownership belongs to the engine, not request clients.
    pub fn observe(&mut self, observer: Arc<dyn RuntimeObserver>) -> Result<(), &'static str> {
        let mut state = self.shared.lock();
        if state.started {
            return Err("query observation is frozen after first request");
        }
        state.observer = Some(observer);
        Ok(())
    }
    pub fn stats(&self) -> RuntimeStats {
        self.shared.lock().stats
    }
    pub fn completion(&self) -> RuntimeCompletion {
        RuntimeCompletion {
            shared: self.shared.clone(),
            caller_stack: self.caller_stack,
        }
    }
    pub fn wait(&mut self, request: RequestId) -> OutcomeList {
        let completion = self.completion();
        loop {
            if let Poll::Ready(result) = self.poll(request) {
                return result;
            }
            completion.wait(request);
        }
    }
    pub fn shutdown(&mut self) {
        drop(self.shutdown_with_reservation());
    }
    /// Join every worker and retire scheduler ownership before returning the
    /// original process charge. A staged caller may join it into its parent
    /// reservation and lend it to another round without a second admission.
    /// Subsequent shutdown calls return None and never release that charge.
    pub fn shutdown_with_reservation(&mut self) -> Option<MemoryReservation> {
        {
            let mut state = self.shared.lock();
            state.stopped = true;
            state.requests.clear();
            state.prune();
            self.shared.changed.notify_all();
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        let _caller = self.shared.caller.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = self.shared.lock();
        state.nodes.clear();
        state.ready.clear();
        state.retained = 0;
        state.stacks = 0;
        state.metadata = 0;
        state.costs.clear();
        state.factories.clear();
        state.observer = None;
        state.account();
        self.reservation.take()
    }
}
type OutcomeList = Result<Vec<ProductRef>, Reference>;
impl Drop for AdmittedRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl QueryRuntime for AdmittedRuntime {
    fn request(&mut self, roots: &[QueryId]) -> RequestId {
        let mut state = self.shared.lock();
        let request = RequestId(state.next_request);
        state.next_request = state
            .next_request
            .checked_add(1)
            .filter(|n| *n < REFUSED_REQUEST)
            .expect("request token exhausted");
        state.started = true;
        if state.stopped {
            return request;
        }
        let root_count: usize = state.requests.values().map(Vec::len).sum();
        if state.requests.len() >= state.max_entries
            || roots.len() > state.max_entries.saturating_sub(root_count)
        {
            return RequestId(request.0 | REFUSED_REQUEST);
        }
        let missing = roots
            .iter()
            .map(|r| key(*r))
            .collect::<BTreeSet<_>>()
            .iter()
            .filter(|r| !state.nodes.contains_key(r))
            .count();
        if missing > state.max_entries.saturating_sub(state.nodes.len()) {
            let idle: Vec<_> = state
                .nodes
                .iter()
                .filter_map(|(&k, n)| {
                    (!n.live
                        && matches!(n.status, Status::Settled(_))
                        && !roots.iter().any(|r| key(*r) == k))
                    .then_some(k)
                })
                .collect();
            for k in idle {
                state.remove(k);
            }
            if missing > state.max_entries.saturating_sub(state.nodes.len()) {
                return RequestId(request.0 | REFUSED_REQUEST);
            }
        }
        let roots: Vec<_> = roots.iter().copied().map(key).collect();
        state.event(RuntimeEvent::Request {
            request,
            roots: roots.iter().copied().map(id).collect(),
        });
        state.requests.insert(request.0, roots.clone());
        for root in roots {
            state.ensure(root).expect("root table pre-admitted");
        }
        state.mark_live();
        self.shared.changed.notify_all();
        request
    }
    fn poll(&mut self, request: RequestId) -> Poll<OutcomeList> {
        if self.caller_stack.is_some() {
            self.completion().wait(request);
        }
        let mut state = self.shared.lock();
        if request.0 & REFUSED_REQUEST != 0 {
            return Poll::Ready(Err(state.errors.resource));
        }
        let Some(roots) = state.requests.get(&request.0) else {
            return Poll::Ready(Err(state.errors.cancelled));
        };
        // A failed early root does not suppress legally required diagnostic roots.
        let results: Option<Vec<Outcome>> = roots.iter().map(|root| state.result(*root)).collect();
        let Some(results) = results else {
            return Poll::Pending;
        };
        state.requests.remove(&request.0);
        state.event(RuntimeEvent::Completed(request));
        state.prune();
        self.shared.changed.notify_all();
        Poll::Ready(results.into_iter().collect())
    }
    fn cancel(&mut self, request: RequestId) {
        let mut state = self.shared.lock();
        if state.requests.remove(&request.0).is_some() {
            state.event(RuntimeEvent::Cancelled(request));
            state.prune();
            self.shared.changed.notify_all();
        }
    }
    fn evict_to(&mut self, retained_bytes: u64) {
        let mut state = self.shared.lock();
        let idle: Vec<_> = state
            .nodes
            .iter()
            .filter_map(|(&key, node)| {
                (!node.live && matches!(node.status, Status::Settled(_))).then_some(key)
            })
            .collect();
        for key in idle {
            if state.retained <= retained_bytes {
                break;
            }
            state.remove(key);
        }
        state.account();
        self.shared.changed.notify_all();
    }
}
impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}
impl State {
    fn event(&mut self, event: RuntimeEvent) {
        if let Some(observer) = &self.observer {
            observer.record(event);
        }
    }
    fn elapsed(&self) -> u64 {
        self.origin.elapsed().as_nanos().min(u64::MAX as u128) as u64
    }
    fn account(&mut self) {
        let bytes = self.stacks + self.metadata + self.retained + self.scratch;
        self.stats.bookkeeping_bytes = self.metadata;
        self.stats.charged_bytes = bytes;
        self.stats.peak_bytes = self.stats.peak_bytes.max(bytes);
        debug_assert!(bytes <= self.budget);
    }
    fn result(&self, key: Key) -> Option<Outcome> {
        match &self.nodes.get(&key)?.status {
            Status::Settled(result) => Some(*result),
            _ => None,
        }
    }
    fn request_complete(&self, request: u64) -> bool {
        self.requests
            .get(&request)
            .is_none_or(|roots| roots.iter().all(|k| self.result(*k).is_some()))
    }
    fn queue(&mut self, key: Key) {
        self.ready
            .insert((Reverse(self.costs.get(&key).copied().unwrap_or(1)), key));
    }
    fn unqueue(&mut self, key: Key) {
        self.ready
            .remove(&(Reverse(self.costs.get(&key).copied().unwrap_or(1)), key));
    }
    fn ensure(&mut self, query: Key) -> Result<(), Reference> {
        if let Some(node) = self.nodes.get_mut(&query) {
            node.live = true;
            return Ok(());
        }
        if self.nodes.len() >= self.max_entries {
            return Err(self.errors.resource);
        }
        self.stats.claims += 1;
        self.event(RuntimeEvent::Claim(id(query)));
        let factory = self.factories.get(&query.0).cloned();
        let created = catch_unwind(AssertUnwindSafe(|| match factory {
            Some(Factory::Send(factory)) => {
                let task = factory.create(id(query))?;
                if task.id() != id(query) {
                    return Err(self.errors.protocol);
                }
                let weight = task.weight();
                let continuation = task.continuation_bytes();
                if continuation > weight.retained {
                    return Err(self.errors.protocol);
                }
                Ok((Task::Send(Some(task)), weight, continuation))
            }
            Some(Factory::Local(factory)) => {
                let weight = factory.weight(id(query))?;
                Ok((
                    Task::Local {
                        factory,
                        home: None,
                    },
                    weight,
                    0,
                ))
            }
            None => Err(self.errors.protocol),
        }))
        .unwrap_or(Err(self.errors.panicked));
        let (task, weight, continuation, status) = match created {
            Ok((task, weight, continuation))
                if weight
                    .scratch
                    .checked_add(weight.retained)
                    .is_some_and(|n| {
                        n <= self.budget.saturating_sub(self.stacks + self.metadata)
                    })
                    && weight.retained
                        <= self.budget.saturating_sub(
                            self.stacks + self.metadata + self.retained + self.scratch,
                        ) =>
            {
                (task, weight, continuation, Status::Ready)
            }
            Ok(_) => (
                Task::Send(None),
                MemoryWeight::default(),
                0,
                Status::Settled(Err(self.errors.resource)),
            ),
            Err(error) => (
                Task::Send(None),
                MemoryWeight::default(),
                0,
                Status::Settled(Err(error)),
            ),
        };
        self.retained += weight.retained;
        let ready = matches!(status, Status::Ready);
        self.nodes.insert(
            query,
            Node {
                task,
                weight,
                continuation,
                status,
                dependencies: BTreeSet::new(),
                unsatisfied: 0,
                waiters: BTreeSet::new(),
                live: true,
                discard: false,
            },
        );
        if ready {
            self.queue(query);
        }
        self.account();
        Ok(())
    }
    fn mark_live(&mut self) {
        for node in self.nodes.values_mut() {
            node.live = false;
        }
        let mut pending: Vec<_> = self.requests.values().flatten().copied().collect();
        while let Some(key) = pending.pop() {
            if let Some(node) = self.nodes.get_mut(&key)
                && !node.live
            {
                node.live = true;
                pending.extend(node.dependencies.iter().copied());
            }
        }
    }
    fn remove(&mut self, query: Key) {
        self.unqueue(query);
        if let Some(node) = self.nodes.remove(&query) {
            // A queued Send continuation still owns its frame until this drop.
            let Node {
                task,
                weight,
                dependencies,
                ..
            } = node;
            drop(task);
            self.retained -= weight.retained;
            self.edges -= dependencies.len();
            for dependency in dependencies {
                if let Some(node) = self.nodes.get_mut(&dependency) {
                    node.waiters.remove(&query);
                }
            }
        }
    }
    fn restart(&mut self, query: Key) {
        // New requesters may subscribe after cancellation but before the old
        // bounded poll/home cleanup returns. Their latch belongs to the logical
        // query, so transfer it to the replacement claim rather than losing it.
        let waiters = std::mem::take(&mut self.nodes.get_mut(&query).expect("old claim").waiters);
        self.remove(query);
        self.ensure(query)
            .expect("replacement uses the same admitted node slot");
        self.nodes.get_mut(&query).unwrap().waiters = waiters;
        if let Some(result) = self.result(query) {
            self.settle(query, result);
        }
    }
    fn prune(&mut self) {
        self.mark_live();
        let mut remove = Vec::new();
        for (&key, node) in &mut self.nodes {
            if !node.live {
                match node.status {
                    Status::Ready | Status::Waiting => match &node.task {
                        Task::Local {
                            home: Some(home), ..
                        } => {
                            self.cleanup[*home].insert(key, None);
                            node.status = Status::Cancelling;
                            node.discard = true;
                        }
                        _ => remove.push(key),
                    },
                    Status::Cancelling => (),
                    Status::Running => node.discard = true,
                    Status::Settled(_) => (),
                }
            }
        }
        for key in remove {
            self.remove(key);
        }
        self.account();
    }
    fn reaches(&self, from: Key, target: Key) -> bool {
        let mut seen = HashSet::new();
        let mut pending = vec![from];
        while let Some(key) = pending.pop() {
            if key == target {
                return true;
            }
            if seen.insert(key)
                && let Some(node) = self.nodes.get(&key)
            {
                pending.extend(node.dependencies.iter().copied());
            }
        }
        false
    }
    /// Called only after the unique task destructor has returned or unwound.
    /// The remaining charge belongs to held products, not the task frame.
    fn retire_continuation(&mut self, query: Key) {
        if let Some(node) = self.nodes.get_mut(&query) {
            let bytes = std::mem::take(&mut node.continuation);
            node.weight.retained -= bytes;
            self.retained -= bytes;
        }
    }
    fn settle(&mut self, query: Key, result: Outcome) {
        let node = self.nodes.get_mut(&query).expect("claimed query");
        node.task = Task::Send(None);
        node.status = Status::Settled(result);
        let waiters = std::mem::take(&mut node.waiters);
        self.stats.settled += 1;
        self.event(RuntimeEvent::Settled {
            query: id(query),
            result,
        });
        for waiter in waiters {
            let Some(node) = self.nodes.get_mut(&waiter) else {
                continue;
            };
            node.unsatisfied -= 1;
            if node.unsatisfied == 0 && matches!(node.status, Status::Waiting) {
                node.status = Status::Ready;
                self.queue(waiter);
                self.stats.wakes += 1;
                self.event(RuntimeEvent::Wake(id(waiter)));
            }
        }
    }
}

struct Context<'a> {
    shared: &'a Shared,
    query: Key,
    pending: BTreeSet<Key>,
}
impl QueryContext for Context<'_> {
    fn read(&mut self, dependency: QueryId) -> Poll<Outcome> {
        let mut state = self.shared.lock();
        if state.stopped
            || state
                .nodes
                .get(&self.query)
                .is_none_or(|n| !n.live || n.discard)
        {
            return Poll::Ready(Err(state.errors.cancelled));
        }
        let dependency = key(dependency);
        let new = !state.nodes[&self.query].dependencies.contains(&dependency);
        if new && state.edges >= state.max_entries * 4 {
            return Poll::Ready(Err(state.errors.resource));
        }
        if let Err(error) = state.ensure(dependency) {
            return Poll::Ready(Err(error));
        }
        if new && state.reaches(dependency, self.query) {
            return Poll::Ready(Err(state.errors.cycle));
        }
        if new {
            state
                .nodes
                .get_mut(&self.query)
                .unwrap()
                .dependencies
                .insert(dependency);
            state.stats.edges += 1;
            state.edges += 1;
            state.event(RuntimeEvent::Read {
                query: id(self.query),
                dependency: id(dependency),
            });
        }
        if let Some(result) = state.result(dependency) {
            return Poll::Ready(result);
        }
        if state
            .nodes
            .get_mut(&dependency)
            .unwrap()
            .waiters
            .insert(self.query)
        {
            state.nodes.get_mut(&self.query).unwrap().unsatisfied += 1;
        }
        self.pending.insert(dependency);
        self.shared.changed.notify_all();
        Poll::Pending
    }
}

fn thread_cpu_ns() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid writable timespec, thread CPU clock has no external state.
    let ok = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
    if ok == 0 {
        (time.tv_sec as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(time.tv_nsec as u64)
    } else {
        0
    }
}
// Tokio tokio-1.48.0 task/local.rs:225,741-805,966 keeps local task
// ownership and destruction on its creating thread. Unlike Tokio, the shared
// query claim/edge table here decides readiness, with a bounded local map and
// home cleanup queue rather than an unbounded remote spawn queue.
fn run_worker(shared: Arc<Shared>, worker: usize, stack: u64, caller: bool) {
    enum Work {
        Send(Box<dyn QueryTask>),
        Local(Arc<dyn LocalQueryFactory>),
    }
    let mut locals: BTreeMap<Key, Box<dyn LocalQueryTask>> = BTreeMap::new();
    loop {
        // Cleanup has priority and needs no scratch permit. Charge stays live
        // until the home worker acknowledges destruction of the continuation.
        let cleanup = shared.lock().cleanup[worker].pop_first();
        if let Some((query, terminal)) = cleanup {
            let dropped = catch_unwind(AssertUnwindSafe(|| {
                locals.remove(&query);
            }));
            let mut state = shared.lock();
            state.retire_continuation(query);
            let live = state.nodes.get(&query).is_some_and(|n| n.live);
            if let Some(error) = terminal {
                let error = if dropped.is_err() {
                    state.errors.panicked
                } else {
                    error
                };
                state.settle(query, Err(error));
            } else {
                if live && !state.stopped {
                    state.restart(query);
                    if dropped.is_err() {
                        let error = state.errors.panicked;
                        state.unqueue(query);
                        state.settle(query, Err(error));
                    }
                } else {
                    state.remove(query);
                }
            }
            state.account();
            shared.changed.notify_all();
            continue;
        }
        let Some((query, work, weight)) = ({
            let mut state = shared.lock();
            loop {
                if !state.cleanup[worker].is_empty() {
                    break None;
                }
                if state.stopped {
                    return;
                }
                let first = state
                    .ready
                    .iter()
                    .find(|(_, query)| match &state.nodes[query].task {
                        Task::Local {
                            home: Some(home), ..
                        } => *home == worker,
                        _ => true,
                    })
                    .copied();
                if let Some((_, query)) = first {
                    let weight = state.nodes[&query].weight;
                    if weight.stack > stack {
                        state.unqueue(query);
                        let error = state.errors.resource;
                        state.settle(query, Err(error));
                        shared.changed.notify_all();
                        continue;
                    }
                    if weight.scratch
                        <= state.budget.saturating_sub(
                            state.stacks + state.metadata + state.retained + state.scratch,
                        )
                    {
                        state.unqueue(query);
                        let node = state.nodes.get_mut(&query).unwrap();
                        let work = match &mut node.task {
                            Task::Send(task) => {
                                Work::Send(task.take().expect("ready continuation"))
                            }
                            Task::Local { factory, home } => {
                                *home = Some(worker);
                                Work::Local(factory.clone())
                            }
                        };
                        node.status = Status::Running;
                        state.scratch += weight.scratch;
                        state.stats.active += 1;
                        state.stats.polls += 1;
                        state.account();
                        let elapsed_ns = state.elapsed();
                        let charged = state.stats.charged_bytes;
                        state.event(RuntimeEvent::Admit {
                            query: id(query),
                            worker,
                            elapsed_ns,
                            charged,
                        });
                        break Some((query, work, weight));
                    }
                    if state.stats.active == 0 {
                        state.unqueue(query);
                        let node = &state.nodes[&query];
                        if let Task::Local {
                            home: Some(home), ..
                        } = &node.task
                        {
                            let home = *home;
                            let error = state.errors.resource;
                            state.cleanup[home].insert(query, Some(error));
                            state.nodes.get_mut(&query).unwrap().status = Status::Cancelling;
                        } else {
                            let error = state.errors.resource;
                            state.settle(query, Err(error));
                        }
                        shared.changed.notify_all();
                        continue;
                    }
                }
                if caller && state.ready.is_empty() {
                    debug_assert!(
                        locals.is_empty(),
                        "idle caller retains a local continuation"
                    );
                    return;
                }
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(|e| e.into_inner());
            }
        }) else {
            continue;
        };
        let start = thread_cpu_ns();
        let mut cx = Context {
            shared: &shared,
            query,
            pending: BTreeSet::new(),
        };
        let mut send_task = None;
        let mut outcome = catch_unwind(AssertUnwindSafe(|| match work {
            Work::Send(mut task) => {
                let outcome = task.poll(&mut cx);
                send_task = Some(task);
                outcome
            }
            Work::Local(factory) => {
                if let std::collections::btree_map::Entry::Vacant(entry) = locals.entry(query) {
                    let task = match factory.create(id(query)) {
                        Ok(task) => task,
                        Err(error) => return QueryPoll::Failed(error),
                    };
                    if task.id() != id(query) || task.weight() != weight {
                        return QueryPoll::Failed(shared.lock().errors.protocol);
                    }
                    let continuation = task.continuation_bytes();
                    if continuation > weight.retained {
                        return QueryPoll::Failed(shared.lock().errors.protocol);
                    }
                    shared
                        .lock()
                        .nodes
                        .get_mut(&query)
                        .expect("running local task")
                        .continuation = continuation;
                    entry.insert(task);
                }
                locals.get_mut(&query).unwrap().poll(&mut cx)
            }
        }));
        // Terminal task frames are destroyed before their charge is retired.
        // Suspended Send/local continuations keep their full admitted weight.
        let keep_local = matches!(outcome, Ok(QueryPoll::Waiting(_)));
        if !keep_local
            && let Err(panic) = catch_unwind(AssertUnwindSafe(|| {
                drop(send_task.take());
                locals.remove(&query);
            }))
        {
            outcome = Err(panic);
        }
        let cpu_ns = thread_cpu_ns().saturating_sub(start);
        let mut state = shared.lock();
        if !keep_local {
            state.retire_continuation(query);
        }
        state.scratch -= weight.scratch;
        state.stats.active -= 1;
        state.account();
        let elapsed_ns = state.elapsed();
        let charged = state.stats.charged_bytes;
        state.event(RuntimeEvent::Release {
            query: id(query),
            worker,
            elapsed_ns,
            cpu_ns,
            charged,
        });
        let discard = state.nodes[&query].discard;
        if discard || state.stopped {
            // Drop without holding the shared graph lock.
            drop(state);
            let _ = catch_unwind(AssertUnwindSafe(|| {
                drop(send_task.take());
                locals.remove(&query);
            }));
            state = shared.lock();
            state.retire_continuation(query);
            // A new request may attach while home-thread destruction runs
            // without the graph lock. Re-read liveness after that handoff.
            let live = state.nodes.get(&query).is_some_and(|node| node.live);
            if live && !state.stopped {
                state.restart(query);
            } else {
                state.remove(query);
            }
        } else {
            match outcome {
                Ok(QueryPoll::Complete(product))
                    if cx.pending.is_empty() && product.address == Address(query.1) =>
                {
                    state.settle(query, Ok(product))
                }
                Ok(QueryPoll::Failed(error)) => state.settle(query, Err(error)),
                Ok(QueryPoll::Waiting(waits))
                    if !cx.pending.is_empty()
                        && waits.iter().copied().map(key).collect::<BTreeSet<_>>()
                            == cx.pending =>
                {
                    let node = state.nodes.get_mut(&query).unwrap();
                    let ready = node.unsatisfied == 0;
                    if let Some(task) = send_task {
                        node.task = Task::Send(Some(task));
                    }
                    node.status = if ready {
                        Status::Ready
                    } else {
                        Status::Waiting
                    };
                    state.stats.waits += 1;
                    state.event(RuntimeEvent::Waiting(id(query)));
                    if ready {
                        state.queue(query);
                        state.stats.wakes += 1;
                        state.event(RuntimeEvent::Wake(id(query)));
                    }
                }
                Err(_) => {
                    let error = state.errors.panicked;
                    state.settle(query, Err(error));
                }
                _ => {
                    drop(state);
                    let _ = catch_unwind(AssertUnwindSafe(|| {
                        drop(send_task.take());
                        locals.remove(&query);
                    }));
                    state = shared.lock();
                    state.retire_continuation(query);
                    let error = state.errors.protocol;
                    state.settle(query, Err(error));
                }
            }
        }
        shared.changed.notify_all();
    }
}
#[cfg(test)]
mod tests;
