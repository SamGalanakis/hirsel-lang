//! The coordinator against a cache-free serial depth-first evaluation, over a
//! model of S1's three request kinds, at jobs 1, 2 and 8, cold and with
//! partial cache hits.
use super::*;
use std::collections::BTreeMap;
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    /// A source constant: bounded; a cycle is CT-4 at the first of the
    /// cycle's sites (`select_diagnostics` order).
    Constant,
    /// A staged constant instance: bounded; a cycle is CT-4 at the request.
    Staged,
    /// A generated declaration: not bounded; requested at its body's site; a
    /// cycle is CT-4 at that site; its failures are memoized.
    Generator,
}

#[derive(Clone, Debug)]
struct Node {
    kind: Kind,
    /// Requests `(node, site)` in order.
    requests: Vec<(usize, u32)>,
    /// The evaluation itself fails before request k (k = len: after all).
    own_failure: Option<usize>,
}

type Graph = Vec<Node>;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum Fail {
    Cycle(u32),
    Overflow(u32),
    Own(usize),
}

fn body_site(node: usize) -> u32 {
    100_000 + node as u32
}
fn root_site(node: usize) -> u32 {
    200_000 + node as u32
}
fn mix(a: u64, b: u64) -> u64 {
    (a ^ b).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(29)
}

/// The cycle diagnostic each kind reports when `node`, at stack position
/// `start`, is requested again from `site`.
fn cycle(kind: Kind, mut sites: impl Iterator<Item = u32>, site: u32) -> Fail {
    match kind {
        Kind::Constant => Fail::Cycle(sites.by_ref().min().unwrap()),
        Kind::Staged | Kind::Generator => Fail::Cycle(site),
    }
}

/// A cache-free serial depth-first evaluation of `node` requested at `site`.
fn reference(
    graph: &Graph,
    limit: u32,
    node: usize,
    stack: &mut Vec<(usize, u32)>,
    site: u32,
) -> Result<u64, Fail> {
    let n = &graph[node];
    let site = if n.kind == Kind::Generator {
        body_site(node)
    } else {
        site
    };
    if let Some(start) = stack.iter().position(|(k, _)| *k == node) {
        let sites: Vec<u32> = stack[start + 1..]
            .iter()
            .map(|(_, s)| *s)
            .chain([site])
            .collect();
        return Err(cycle(n.kind, sites.into_iter(), site));
    }
    if n.kind != Kind::Generator && stack.len() as u32 >= limit {
        return Err(Fail::Overflow(site));
    }
    stack.push((node, site));
    let result = (|| {
        let mut value = node as u64;
        for (k, &(child, s)) in n.requests.iter().enumerate() {
            if n.own_failure == Some(k) {
                return Err(Fail::Own(node));
            }
            value = mix(value, reference(graph, limit, child, stack, s)?);
        }
        if n.own_failure == Some(n.requests.len()) {
            return Err(Fail::Own(node));
        }
        Ok(value)
    })();
    stack.pop();
    result
}

/// A memo entry: a value or a stored failure, with its evidence.
type Entry = (Result<u64, Fail>, DepthEvidence<u32>);

struct Model {
    graph: Graph,
    limit: u32,
    memo: Mutex<HashMap<usize, Entry>>,
    /// Nodes whose first evaluation waits for the others' (forces
    /// cross-worker cycles).
    rendezvous: Option<(Barrier, Vec<usize>)>,
    entered: Vec<AtomicBool>,
    /// Pause per evaluation, to vary interleavings.
    jitter: u64,
}

impl Model {
    fn new(graph: Graph, limit: u32) -> Self {
        let entered = graph.iter().map(|_| AtomicBool::new(false)).collect();
        Self {
            graph,
            limit,
            memo: Mutex::new(HashMap::new()),
            rendezvous: None,
            entered,
            jitter: 0,
        }
    }

    /// The engine's request path: memo, cycle, depth, claim, evaluate.
    fn request(&self, w: &mut S1Worker<usize, u32>, node: usize, site: u32) -> Result<u64, Fail> {
        let n = &self.graph[node];
        let site = if n.kind == Kind::Generator {
            body_site(node)
        } else {
            site
        };
        loop {
            let hit = self.memo.lock().unwrap().get(&node).cloned();
            if let Some((result, evidence)) = hit {
                w.admit(&evidence, site).map_err(Fail::Overflow)?;
                w.requested(&evidence, site);
                return result;
            }
            if let Some(start) = w.on_stack(&node) {
                w.fail_cycle();
                return Err(cycle(n.kind, w.cycle_sites(start, site), site));
            }
            if n.kind != Kind::Generator {
                w.admit(&DepthEvidence::leaf(self.limit), site)
                    .map_err(Fail::Overflow)?;
            }
            match w.enter(node, site) {
                Entered::Run => break,
                Entered::Settled => continue,
            }
        }
        if let Some((barrier, nodes)) = &self.rendezvous
            && nodes.contains(&node)
            && !self.entered[node].swap(true, Ordering::SeqCst)
        {
            barrier.wait();
        }
        for _ in 0..self.jitter {
            std::thread::yield_now();
        }
        let result = (|| {
            let mut value = node as u64;
            for (k, &(child, s)) in n.requests.iter().enumerate() {
                if n.own_failure == Some(k) {
                    return Err(Fail::Own(node));
                }
                value = mix(value, self.request(w, child, s)?);
            }
            if n.own_failure == Some(n.requests.len()) {
                return Err(Fail::Own(node));
            }
            Ok(value)
        })();
        let bounded = n.kind != Kind::Generator;
        let evidence = w.composed(self.limit, bounded);
        // Store before leaving, so a waiter finds it; a failure is stored
        // only by a generator, and only when it is the generator's own.
        let own = w.failure_is_own();
        if result.is_ok() || (n.kind == Kind::Generator && own) {
            self.memo
                .lock()
                .unwrap()
                .entry(node)
                .or_insert((result, evidence.clone()));
        }
        w.leave();
        w.requested(&evidence, site);
        result
    }
}

/// Every node as a root, in `order`, on `jobs` workers; outcomes by root.
fn run(
    model: &Model,
    jobs: usize,
    order: &[usize],
) -> (BTreeMap<usize, Result<u64, Fail>>, CoordinatorStats) {
    let coordinator = S1Coordinator::new();
    let next = AtomicUsize::new(0);
    let outcomes = Mutex::new(BTreeMap::new());
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                let mut worker = coordinator.worker();
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(&root) = order.get(i) else { break };
                    let outcome = model.request(&mut worker, root, root_site(root));
                    assert!(worker.is_empty());
                    outcomes.lock().unwrap().insert(root, outcome);
                }
            });
        }
    });
    (outcomes.into_inner().unwrap(), coordinator.stats())
}

/// Runs `body` on another thread, failing on a deadlock.
fn within<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || send.send(body()).unwrap());
    receive
        .recv_timeout(Duration::from_secs(60))
        .expect("the pool deadlocked or a worker panicked")
}

fn expected(graph: &Graph, limit: u32) -> BTreeMap<usize, Result<u64, Fail>> {
    (0..graph.len())
        .map(|root| {
            (
                root,
                reference(graph, limit, root, &mut vec![], root_site(root)),
            )
        })
        .collect()
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn random_graph(rng: &mut Rng, size: usize, cyclic: bool) -> Graph {
    (0..size)
        .map(|node| {
            let kind = match rng.below(4) {
                0 => Kind::Staged,
                1 => Kind::Generator,
                _ => Kind::Constant,
            };
            let mut requests = vec![];
            for k in 0..rng.below(4) {
                // Mostly forward edges; backward ones make cycles.
                let child = if cyclic && rng.below(6) == 0 {
                    rng.below(size)
                } else if node + 1 < size {
                    node + 1 + rng.below(size - node - 1)
                } else {
                    continue;
                };
                requests.push((child, (node * 100 + k) as u32));
            }
            let own_failure = (rng.below(10) == 0).then(|| rng.below(requests.len() + 1));
            Node {
                kind,
                requests,
                own_failure,
            }
        })
        .collect()
}

fn shuffled(rng: &mut Rng, size: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..size).collect();
    for i in (1..size).rev() {
        order.swap(i, rng.below(i + 1));
    }
    order
}

/// Random graphs with cycles, own failures, all three kinds and small
/// limits: every root's outcome at jobs 1/2/8, cold and over a memo warmed by
/// a subset of roots, equals the cache-free serial evaluation.
#[test]
fn random_graphs_match_serial_at_every_job_count() {
    let mut rng = Rng(0x5eed_c1c1_0000_0001);
    for round in 0..300 {
        let size = 2 + rng.below(14);
        let limit = 1 + rng.below(6) as u32;
        let graph = random_graph(&mut rng, size, round % 3 != 0);
        let want = expected(&graph, limit);
        for jobs in [1, 2, 8] {
            for warm in [false, true] {
                let mut model = Model::new(graph.clone(), limit);
                model.jitter = rng.below(50) as u64;
                if warm {
                    let some: Vec<usize> = shuffled(&mut rng, size)
                        .into_iter()
                        .take(1 + rng.below(size))
                        .collect();
                    run(&model, 1, &some);
                }
                let order = shuffled(&mut rng, size);
                let model = Arc::new(model);
                let shared = model.clone();
                let (got, _) = within(move || run(&shared, jobs, &order));
                assert_eq!(
                    got, want,
                    "round {round}, jobs {jobs}, warm {warm}, limit {limit}, graph {graph:?}"
                );
            }
        }
    }
}

/// Two tasks each own one side of a constant cycle and request the other:
/// one parks, the other would close a wait-for cycle and runs the request on
/// its own stack. Both report the serial CT-4.
#[test]
fn two_task_cycle_reports_the_serial_diagnostic() {
    let graph = vec![
        Node {
            kind: Kind::Constant,
            requests: vec![(1, 7)],
            own_failure: None,
        },
        Node {
            kind: Kind::Constant,
            requests: vec![(0, 3)],
            own_failure: None,
        },
    ];
    let want = expected(&graph, 8);
    assert_eq!(want[&0], Err(Fail::Cycle(3)));
    assert_eq!(want[&1], Err(Fail::Cycle(3)));
    for _ in 0..20 {
        let mut model = Model::new(graph.clone(), 8);
        model.rendezvous = Some((Barrier::new(2), vec![0, 1]));
        let model = Arc::new(model);
        let (got, stats) = within(move || run(&model, 2, &[0, 1]));
        assert_eq!(got, want);
        assert!(stats.parks >= 1 && stats.duplicates >= 1, "{stats:?}");
    }
}

/// A constant → generator → staged → constant cycle entered by three tasks at
/// once, and the same cycle entered at each point serially.
#[test]
fn mixed_three_task_cycle_matches_serial() {
    let graph = vec![
        Node {
            kind: Kind::Constant,
            requests: vec![(1, 10)],
            own_failure: None,
        },
        Node {
            kind: Kind::Generator,
            requests: vec![(2, 20)],
            own_failure: None,
        },
        Node {
            kind: Kind::Staged,
            requests: vec![(0, 30)],
            own_failure: None,
        },
        // Leans on the cycle from outside.
        Node {
            kind: Kind::Constant,
            requests: vec![(2, 40)],
            own_failure: None,
        },
    ];
    let want = expected(&graph, 8);
    for jobs in [1, 2, 3, 8] {
        for _ in 0..10 {
            let mut model = Model::new(graph.clone(), 8);
            if jobs >= 3 {
                model.rendezvous = Some((Barrier::new(3), vec![0, 1, 2]));
            }
            let model = Arc::new(model);
            let (got, _) = within(move || run(&model, jobs, &[0, 1, 2, 3]));
            assert_eq!(got, want, "jobs {jobs}");
        }
    }
}

/// Every worker requests the same long chain at once: all but one park on
/// the chain's head, and depth is decided from evidence, so a root just under
/// the limit succeeds and one just over it fails at the serial site.
#[test]
fn saturated_chain_parks_and_keeps_serial_depth() {
    let limit = 32;
    let len = 40;
    let mut graph: Graph = (0..len)
        .map(|i| Node {
            kind: if i % 5 == 4 {
                Kind::Generator
            } else {
                Kind::Constant
            },
            requests: if i + 1 < len {
                vec![(i + 1, i as u32)]
            } else {
                vec![]
            },
            own_failure: None,
        })
        .collect();
    // Eight roots on the chain's head, each one link from it.
    for r in 0..8 {
        graph.push(Node {
            kind: Kind::Constant,
            requests: vec![(len - 30 + r, 500 + r as u32)],
            own_failure: None,
        });
    }
    let want = expected(&graph, limit);
    assert!(want.values().any(|r| r.is_ok()));
    assert!(want.values().any(|r| matches!(r, Err(Fail::Overflow(_)))));
    for jobs in [1, 2, 8] {
        for warm in [None, Some(len - 10), Some(len - 25)] {
            let model = Model::new(graph.clone(), limit);
            if let Some(w) = warm {
                run(&model, 1, &[w]);
            }
            let model = Arc::new(model);
            let order: Vec<usize> = (len..len + 8).chain(0..len).collect();
            let (got, _) = within(move || run(&model, jobs, &order));
            assert_eq!(got, want, "jobs {jobs}, warm {warm:?}");
        }
    }
}

/// A generator's result requested at or past the limit: it is not bounded
/// itself, so a leaf generator succeeds where a bounded request would fail,
/// warm and cold alike.
#[test]
fn unbounded_generator_evidence_matches_cold() {
    // 0 (constant) → 1 (constant) → 2 (generator, leaf); limit 2.
    let graph = vec![
        Node {
            kind: Kind::Constant,
            requests: vec![(1, 1)],
            own_failure: None,
        },
        Node {
            kind: Kind::Constant,
            requests: vec![(2, 2)],
            own_failure: None,
        },
        Node {
            kind: Kind::Generator,
            requests: vec![],
            own_failure: None,
        },
    ];
    let want = expected(&graph, 2);
    assert!(want[&0].is_ok(), "{want:?}");
    for warm in [vec![], vec![1], vec![2], vec![2, 1]] {
        let model = Model::new(graph.clone(), 2);
        run(&model, 1, &warm);
        assert_eq!(run(&model, 1, &[0, 1, 2]).0, want, "warm {warm:?}");
    }
}

/// A worker dropped mid-evaluation releases its claims and wakes waiters.
#[test]
fn dropped_worker_releases_its_claims() {
    let coordinator = S1Coordinator::<usize>::new();
    let mut a = coordinator.worker::<u32>();
    assert_eq!(a.enter(1, 0), Entered::Run);
    let waiter = {
        let coordinator = coordinator.clone();
        std::thread::spawn(move || {
            let mut b = coordinator.worker::<u32>();
            b.enter(1, 0)
        })
    };
    while coordinator.stats().parks == 0 {
        std::thread::yield_now();
    }
    drop(a);
    assert_eq!(waiter.join().unwrap(), Entered::Settled);
}
