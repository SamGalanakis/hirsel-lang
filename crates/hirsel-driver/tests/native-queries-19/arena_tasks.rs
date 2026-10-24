use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::ResultStore, *};
use hirsel_syntax::ContentHash;
use native::queries::{
    ARENA_CANDIDATES_KIND, ARENA_CHOSEN_KIND, INSTANCE_KIND,
    arena_tasks::{ARENA_GRAPH_KIND, ArenaTaskFactory, ArenaTasks},
    declarations::NativeDeclarations,
    instance_tasks::{DISCOVERY_KIND, INSTANCE_INPUT_KIND, InstanceTaskFactory, InstanceTasks},
    local::LocalReadyQueries,
    ready::EMISSION_CONTEXT_DOMAIN,
    source_inputs::NativeInstanceInputs,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, atomic::Ordering},
    task::Poll,
};

#[derive(Default)]
struct Events(Mutex<Vec<RuntimeEvent>>);
impl RuntimeObserver for Events {
    fn record(&self, event: RuntimeEvent) {
        self.0.lock().unwrap().push(event);
    }
}
struct Traced(Arc<dyn QueryFactory>, hirsel_lir::construction::ValueTrace);
struct Task(Box<dyn QueryTask>, hirsel_lir::construction::ValueTrace);
impl QueryFactory for Traced {
    fn create(&self, key: QueryId) -> Result<Box<dyn QueryTask>, store::Reference> {
        Ok(Box::new(Task(self.0.create(key)?, self.1.clone())))
    }
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.0.id()
    }
    fn weight(&self) -> MemoryWeight {
        self.0.weight()
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        let _guard = self.1.enter();
        self.0.poll(cx)
    }
}
struct Pending;
impl QueryContext for Pending {
    fn read(&mut self, _: QueryId) -> Poll<Result<ProductRef, store::Reference>> {
        Poll::Pending
    }
}
struct Failed(store::Reference);
impl QueryContext for Failed {
    fn read(&mut self, _: QueryId) -> Poll<Result<ProductRef, store::Reference>> {
        Poll::Ready(Err(self.0))
    }
}

#[test]
fn cold_arena_tasks_preserve_both_strata_and_recursive_callee_invalidation_at_widths_one_and_four()
{
    with_compiler_stack(|| {
        const BEFORE: &str = "type Item { n: u64 }\nfn first(a: Arena, n: u64) -> (box Item) in a {\n  if n == 0u64 { box Item(n:6u64) }\n  else { second(a,n-1u64) }\n}\nfn second(a: Arena, n: u64) -> (box Item) in a { first(a,n) }\nfn wrapper(a: Arena) -> (box Item) in a { second(a,2u64) }\nfn main() { arena.with(fn(a: Arena) { let item = wrapper(a); }); }\n";
        const AFTER: &str = "type Item { n: u64 }\nfn first(a: Arena, n: u64) -> (box Item) in a {\n  if n == 0u64 { let h: (box Item) in heap = box Item(n:6u64); h }\n  else { second(a,n-1u64) }\n}\nfn second(a: Arena, n: u64) -> (box Item) in a { first(a,n) }\nfn wrapper(a: Arena) -> (box Item) in a { second(a,2u64) }\nfn main() { arena.with(fn(a: Arena) { let item = wrapper(a); }); }\n";
        let stable = BEFORE.replace("6u64", "7u64");
        let mut deterministic = BTreeMap::new();
        for jobs in [1, 4] {
            let directory = tempfile::tempdir().unwrap();
            let store = Arc::new(store::DirStore::new(directory.path().join("products")));
            let emission = store.put_content(EMISSION_CONTEXT_DOMAIN, b"{}").unwrap();
            let mut previous = BTreeMap::new();
            let mut previous_graph = None;
            for (iteration, (source, fresh)) in [
                (BEFORE, false),
                (BEFORE, true),
                (stable.as_str(), false),
                (AFTER, false),
                (AFTER, true),
            ]
            .into_iter()
            .enumerate()
            {
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                    .with_result_store(store.clone());
                let file = engine
                    .sources
                    .add("scheduled-arena.hir", source.into())
                    .unwrap();
                engine.parse_file(file, &default_context()).unwrap();
                if fresh {
                    engine.adapters.lowering = None;
                }
                let mut source_queries =
                    NativeDiscoveryCache::new(engine.types().clone(), store.clone());
                let mut declarations =
                    NativeDeclarations::collect(&mut engine, &mut source_queries, &[]).unwrap();
                if fresh {
                    assert_eq!(source_queries.counts.body_demands, 0);
                }
                let mut originals = Vec::new();
                if !fresh {
                    for owner in declarations
                        .context
                        .sources
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                    {
                        let body = declarations.body(&mut engine, &owner).unwrap();
                        originals.push(Arc::downgrade(&body));
                    }
                }
                let operations = declarations.program.operation_registry();
                let manifest = hirsel_core_contract::Manifest::skeleton(
                    declarations.program.operations.values().cloned(),
                );
                let source = NativeInstanceInputs::capture(&mut engine, declarations)
                    .unwrap()
                    .unwrap();
                drop(engine);
                assert!(originals.iter().all(|body| body.upgrade().is_none()));
                let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
                let options = native::Options::new(target.clone());
                let budget = ProcessBudget::new(768 << 20);
                let weight = MemoryWeight {
                    scratch: 32 << 20,
                    retained: 1 << 20,
                    stack: 0,
                };
                let trace = hirsel_lir::construction::ValueTrace::default();
                let _guard = trace.enter();
                let instances = InstanceTasks::new(
                    source,
                    store.clone(),
                    options.clone(),
                    ContentHash::of_bytes(b"cold arena control"),
                    weight,
                    budget.reserve(32 << 20).unwrap(),
                )
                .unwrap();
                let arena = ArenaTasks::new(
                    instances.clone(),
                    store.clone(),
                    0,
                    weight,
                    budget.reserve(32 << 20).unwrap(),
                )
                .unwrap();
                let instance_factory = Arc::new(InstanceTaskFactory::new(instances.clone()));
                let arena_factory = Arc::new(ArenaTaskFactory::new(arena.clone()));
                let mut lazy = arena_factory.create(arena.query_id()).unwrap();
                assert!(
                    matches!(lazy.poll(&mut Pending), QueryPoll::Waiting(ref reads) if reads == &vec![instances.discovery_id()])
                );
                assert_eq!(arena.solves.load(Ordering::Relaxed), 0);
                assert_eq!(instances.imports.load(Ordering::Relaxed), 0);
                assert_eq!(trace.totals(), Default::default());
                let denied = instances.failure("discovery refused");
                assert!(
                    matches!(lazy.poll(&mut Failed(denied)), QueryPoll::Failed(error) if error == denied)
                );
                drop(lazy);
                let errors = RuntimeErrors {
                    resource: denied,
                    cycle: denied,
                    cancelled: denied,
                    protocol: denied,
                    panicked: denied,
                };
                let mut runtime = AdmittedRuntime::new(
                    hirsel_intern::jobs::Jobs::new(jobs),
                    budget.reserve(704 << 20).unwrap(),
                    errors,
                )
                .unwrap();
                let instance_factory = Arc::new(Traced(instance_factory, trace.clone()));
                let arena_factory = Arc::new(Traced(arena_factory, trace.clone()));
                for kind in [INSTANCE_KIND, INSTANCE_INPUT_KIND, DISCOVERY_KIND] {
                    runtime.register(kind, instance_factory.clone()).unwrap();
                }
                for kind in [ARENA_CANDIDATES_KIND, ARENA_CHOSEN_KIND, ARENA_GRAPH_KIND] {
                    runtime.register(kind, arena_factory.clone()).unwrap();
                }
                let events = Arc::new(Events::default());
                runtime.observe(events.clone()).unwrap();
                let request = runtime.request(&[arena.query_id(), instances.discovery_id()]);
                let selected = runtime.wait(request).unwrap_or_else(|error| {
                    panic!(
                        "{}",
                        String::from_utf8_lossy(&store.get_content(error).unwrap())
                    )
                });
                let completed = instances.completed_instances(selected[1]).unwrap();
                let base = instances.products(selected[1]).unwrap();
                let graph = arena.graph(selected[0]).unwrap();
                // The concrete recursive witness retains exactly one component
                // containing first/second in each stratum and propagates its
                // changed choice into the unchanged wrapper.
                assert_eq!(
                    graph
                        .component_keys
                        .iter()
                        .filter(|key| key.members.iter().any(|owner| owner
                            .instance
                            .definition()
                            .0
                            .name
                            == "first")
                            && key
                                .members
                                .iter()
                                .any(|owner| owner.instance.definition().0.name == "second"))
                        .count(),
                    2
                );
                let wrapper = completed
                    .iter()
                    .find(|(key, _)| key.instance.definition().0.name == "wrapper")
                    .unwrap();
                let summary = graph.chosen_reads(&wrapper.0, &[]).unwrap();
                let claim = summary[0]
                    .candidates
                    .iter()
                    .find(|(key, _)| key == &wrapper.0)
                    .unwrap();
                assert_eq!(claim.1, if iteration < 3 { vec![0] } else { vec![] });
                let mut ready = LocalReadyQueries::new(store.clone());
                let environment = hirsel_lir::validate::NativeEnvironment {
                    target: &target,
                    operations: &operations,
                    manifest: &manifest,
                    discharge: Some(&options.discharge),
                };
                let mut links = Vec::new();
                let mut actual = BTreeMap::new();
                for (key, output) in &completed {
                    let product = ready
                        .ready(&base, key, &graph, &options, emission, true, &environment)
                        .unwrap();
                    links.push(
                        ready
                            .queries
                            .linkage(key, product.product.action, &environment)
                            .unwrap(),
                    );
                    actual.insert(
                        (key.instance.definition().clone(), key.instance.digest()),
                        (output.product.product.record, product.product.record),
                    );
                }
                hirsel_lir::validate::check_arena_scoped_links(&links).unwrap();
                if fresh {
                    assert_eq!(actual, previous);
                    assert_eq!(Some(selected[0]), previous_graph);
                    assert_eq!(
                        (
                            instances.imports.load(Ordering::Relaxed),
                            arena.solves.load(Ordering::Relaxed),
                            ready.queries.counts.transformations
                        ),
                        (0, 0, 0)
                    );
                    assert_eq!(trace.totals(), Default::default());
                } else if iteration == 2 {
                    assert_eq!(
                        (
                            instances.imports.load(Ordering::Relaxed),
                            arena.solves.load(Ordering::Relaxed),
                            ready.queries.counts.transformations
                        ),
                        (1, 0, 1)
                    );
                    assert_eq!(
                        Some(selected[0]),
                        previous_graph,
                        "equal equation outputs cut off both strata"
                    );
                    assert!(
                        trace
                            .snapshot()
                            .keys()
                            .all(|(path, _)| path.0.name == "first")
                    );
                } else if iteration == 3 {
                    assert_eq!(instances.imports.load(Ordering::Relaxed), 1);
                    assert!(arena.solves.load(Ordering::Relaxed) >= 4);
                    assert!(ready.queries.counts.transformations >= 3);
                    for (owner, products) in &actual {
                        if ["second", "wrapper"].contains(&owner.0.0.name.as_str()) {
                            assert_eq!(previous[owner].0, products.0);
                            assert_ne!(previous[owner].1, products.1);
                        }
                    }
                }
                if jobs == 1 {
                    deterministic.insert(iteration, (actual.clone(), selected[0]));
                } else {
                    assert_eq!(deterministic[&iteration], (actual.clone(), selected[0]));
                }
                let events = events.0.lock().unwrap();
                assert!(events.iter().any(|event| matches!(event, RuntimeEvent::Read {query, dependency} if query.kind == ARENA_CHOSEN_KIND && dependency.kind == ARENA_CANDIDATES_KIND)));
                assert!(events.iter().any(|event| matches!(event, RuntimeEvent::Read {query, dependency} if query.kind == ARENA_CHOSEN_KIND && dependency.kind == ARENA_CHOSEN_KIND)));
                assert!(events.iter().all(|event| !matches!(event, RuntimeEvent::Read {query, dependency} if query.kind == ARENA_CANDIDATES_KIND && dependency.kind == ARENA_CHOSEN_KIND)));
                assert!(events.iter().any(|event| matches!(event, RuntimeEvent::Read {query, dependency} if query.kind == ARENA_CANDIDATES_KIND && dependency.kind == INSTANCE_KIND)));
                drop(events);
                drop(runtime);
                drop(instance_factory);
                drop(arena_factory);
                assert_eq!(budget.reserved(), 64 << 20);
                drop(arena);
                drop(instances);
                assert_eq!(budget.reserved(), 0);
                previous = actual;
                previous_graph = Some(selected[0]);
            }
        }
    });
}
