use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::ResultStore, *};
use hirsel_syntax::ContentHash;
use native::queries::{
    INSTANCE_KIND,
    declarations::NativeDeclarations,
    instance_tasks::{DISCOVERY_KIND, INSTANCE_INPUT_KIND, InstanceTaskFactory, InstanceTasks},
    source_inputs::NativeInstanceInputs,
};
use std::{
    collections::{BTreeMap, BTreeSet},
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
struct TraceFactory {
    inner: Arc<InstanceTaskFactory>,
    trace: hirsel_lir::construction::ValueTrace,
}
struct TraceTask {
    inner: Box<dyn QueryTask>,
    trace: hirsel_lir::construction::ValueTrace,
}
impl QueryFactory for TraceFactory {
    fn create(&self, key: QueryId) -> Result<Box<dyn QueryTask>, store::Reference> {
        Ok(Box::new(TraceTask {
            inner: self.inner.create(key)?,
            trace: self.trace.clone(),
        }))
    }
}
impl QueryTask for TraceTask {
    fn id(&self) -> QueryId {
        self.inner.id()
    }
    fn weight(&self) -> MemoryWeight {
        self.inner.weight()
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        let _observe = self.trace.enter();
        self.inner.poll(cx)
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
fn run(
    source: &str,
    store: Arc<store::DirStore>,
    jobs: usize,
    fresh: bool,
) -> (
    BTreeMap<Vec<u8>, ProductRef>,
    ProductRef,
    u64,
    BTreeSet<String>,
) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
        .with_result_store(store.clone());
    let file = engine
        .sources
        .add("task-demand.hir", source.into())
        .unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    if fresh {
        engine.adapters.lowering = None;
    }
    let mut discovery = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
    let mut declarations = NativeDeclarations::collect(&mut engine, &mut discovery, &[]).unwrap();
    if fresh {
        assert_eq!(discovery.counts.body_demands, 0);
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
    let source = NativeInstanceInputs::capture(&mut engine, declarations)
        .unwrap()
        .unwrap();
    drop(engine);
    assert!(originals.iter().all(|body| body.upgrade().is_none()));
    let options = native::Options::new(hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu());
    let policy = ContentHash::of_bytes(b"cold instance runtime control");
    let budget = ProcessBudget::new(768 << 20);
    let weight = MemoryWeight {
        scratch: 32 << 20,
        retained: 1 << 20,
        stack: 0,
    };
    let trace = hirsel_lir::construction::ValueTrace::default();
    let _observe = trace.enter();
    let tasks = InstanceTasks::new(
        source,
        store.clone(),
        options,
        policy,
        weight,
        budget.reserve(32 << 20).unwrap(),
    )
    .unwrap();
    assert_eq!(trace.totals(), Default::default());
    let inner = Arc::new(InstanceTaskFactory::new(tasks.clone()));
    let root = &tasks.roots()[0];
    let root_id = QueryId {
        kind: INSTANCE_KIND,
        address: root.address(INSTANCE_KIND),
    };
    let mut lazy = inner.create(root_id).unwrap();
    assert!(
        matches!(lazy.poll(&mut Pending), QueryPoll::Waiting(ref keys)
        if keys == &vec![QueryId {kind: INSTANCE_INPUT_KIND, address: root.address(INSTANCE_INPUT_KIND)}])
    );
    assert_eq!(tasks.imports.load(Ordering::Relaxed), 0);
    assert_eq!(tasks.source_body_demands.load(Ordering::Relaxed), 0);
    assert_eq!(trace.totals(), Default::default());
    let denied = tasks.failure("source producer refused");
    assert!(matches!(lazy.poll(&mut Failed(denied)), QueryPoll::Failed(error) if error == denied));
    // A failed local continuation has published neither a native output nor a
    // discovery completion. Runtime construction below starts fresh frames.
    assert!(
        tasks
            .products(ProductRef {
                address: tasks.discovery_id().address,
                action: hirsel_artifact_store::Action(policy),
                record: denied
            })
            .is_err()
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
        budget.reserve(736 << 20).unwrap(),
        errors,
    )
    .unwrap();
    let factory = Arc::new(TraceFactory {
        inner,
        trace: trace.clone(),
    });
    for kind in [INSTANCE_KIND, INSTANCE_INPUT_KIND, DISCOVERY_KIND] {
        runtime.register(kind, factory.clone()).unwrap();
    }
    let events = Arc::new(Events::default());
    runtime.observe(events.clone()).unwrap();
    let request = runtime.request(&[tasks.discovery_id(), root_id]);
    let result = runtime.wait(request).unwrap_or_else(|error| {
        panic!(
            "{}",
            String::from_utf8_lossy(&store.get_content(error).unwrap())
        )
    });
    let completed = tasks.completed_instances(result[0]).unwrap();
    let products = completed
        .iter()
        .map(|(key, output)| (key.instance.encoding().to_vec(), output.product.product))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(products[root.instance.encoding()], result[1]);
    let before = trace.totals();
    let assembled = tasks.products(result[0]).unwrap();
    assert_eq!(
        trace.totals(),
        before,
        "assembly has no dependency or native work"
    );
    assert_eq!(assembled.imports, 0);
    let imports = tasks.imports.load(Ordering::Relaxed);
    assert_eq!(tasks.recipe_decodes.load(Ordering::Relaxed), imports);
    if fresh {
        assert_eq!(imports, 0);
        assert_eq!(tasks.source_body_demands.load(Ordering::Relaxed), 0);
        assert_eq!(trace.totals(), Default::default());
    }
    let events = events.0.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::Claim(key) if *key == root_id))
            .count(),
        1
    );
    assert!(events.iter().any(|event| matches!(event, RuntimeEvent::Read {query, dependency} if query.kind == DISCOVERY_KIND && dependency.kind == INSTANCE_KIND)));
    assert!(events.iter().any(|event| matches!(event, RuntimeEvent::Read {query, dependency} if query.kind == INSTANCE_KIND && dependency.kind == INSTANCE_INPUT_KIND)));
    assert!(events.iter().all(|event| !matches!(event, RuntimeEvent::Read {query, dependency} if query.kind == INSTANCE_KIND && dependency.kind == INSTANCE_KIND)), "recursive discovery never waits on callee bodies");
    drop(events);
    drop(runtime);
    assert_eq!(
        budget.reserved(),
        32 << 20,
        "registry reservation outlives the worker pool"
    );
    let owners = trace
        .snapshot()
        .keys()
        .map(|(path, _)| path.0.name.to_string())
        .collect();
    drop(assembled);
    drop(factory);
    drop(tasks);
    assert_eq!(budget.reserved(), 0);
    (products, result[0], imports, owners)
}

#[test]
fn cold_instance_tasks_discover_generics_closures_finalizers_and_recursion_at_widths_one_and_four()
{
    with_compiler_stack(|| {
        const SOURCE: &str = "resource type R[T:copyable] {n:T} drop(x) {let _=x.n}\nfn outer[T](take x:T)->T {let f:once fn()->T=fn()->T{take x};f()}\nfn recur(n:u64)->u64 {if n==0u64 {0u64} else {recur(n-1u64)}}\nfn unrelated()->u64{13u64}\npub fn main()->u64 {let r=R(n:1u64);outer(take 7u64)+recur(1u64)}";
        let mut reference = None;
        for jobs in [1, 4] {
            let directory = tempfile::tempdir().unwrap();
            let store = Arc::new(store::DirStore::new(directory.path().join("products")));
            let (cold, root, imports, _) = run(SOURCE, store.clone(), jobs, false);
            assert!(imports >= 6);
            if let Some((expected, expected_root)) = &reference {
                assert_eq!(&cold, expected);
                assert_eq!(&root, expected_root);
            } else {
                reference = Some((cold.clone(), root));
            }
            let (fresh, fresh_root, _, _) = run(SOURCE, store.clone(), jobs, true);
            assert_eq!(fresh, cold);
            assert_eq!(fresh_root, root);
            let changed = SOURCE.replace("13u64", "14u64");
            let (edit, edit_root, imports, owners) = run(&changed, store.clone(), jobs, false);
            assert_eq!(imports, 1);
            assert_eq!(owners, BTreeSet::from(["unrelated".into()]));
            assert_eq!(
                cold.iter()
                    .filter(|(key, product)| edit[*key] != **product)
                    .count(),
                1
            );
            assert_ne!(edit_root, root);
            let (fresh, fresh_root, _, _) = run(&changed, store, jobs, true);
            assert_eq!(fresh, edit);
            assert_eq!(fresh_root, edit_root);
        }
    });
}

#[test]
fn cold_instance_tasks_register_shared_library_environment_requests_without_duplicate_imports() {
    with_compiler_stack(|| {
        const SOURCE: &str = "fn step(mut n:u64)->Option[u64] {if n==2u64 {None} else {let x=n;n+=1u64;Some(x)}}\npub fn main()->u64 {let state=0u64;let mut it=iter.from(take state,step);let _=iter.next(mut it);0u64}";
        for jobs in [1, 4] {
            let directory = tempfile::tempdir().unwrap();
            let store = Arc::new(store::DirStore::new(directory.path().join("products")));
            let (cold, root, imports, owners) = run(SOURCE, store.clone(), jobs, false);
            assert_eq!(imports as usize, cold.len());
            assert!(owners.contains("iter_from_step"));
            let (fresh, fresh_root, _, _) = run(SOURCE, store, jobs, true);
            assert_eq!(fresh, cold);
            assert_eq!(fresh_root, root);
        }
    });
}
