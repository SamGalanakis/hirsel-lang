use hirsel_driver::*;
use hirsel_engine::{runtime::*, *};
use hirsel_syntax::ContentHash;
use native::queries::{
    INSTANCE_KIND,
    declarations::NativeDeclarations,
    instance_tasks::{DISCOVERY_KIND, INSTANCE_INPUT_KIND, InstanceTaskFactory, InstanceTasks},
    operation::OPERATION_COMPONENT_KIND,
    operation_tasks::{OPERATION_GRAPH_KIND, OperationTaskFactory, OperationTasks},
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
struct Traced(Arc<dyn QueryFactory>, hirsel_lir::construction::ValueTrace);
struct TracedTask(Box<dyn QueryTask>, hirsel_lir::construction::ValueTrace);
impl QueryFactory for Traced {
    fn create(&self, key: QueryId) -> Result<Box<dyn QueryTask>, store::Reference> {
        Ok(Box::new(TracedTask(self.0.create(key)?, self.1.clone())))
    }
}
impl QueryTask for TracedTask {
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
fn cold_operation_tasks_read_actual_instance_and_component_outputs_at_widths_one_and_four() {
    with_compiler_stack(|| {
        for jobs in [1, 4] {
            run(jobs);
        }
    });
}
fn run(jobs: usize) {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(store::DirStore::new(directory.path().to_path_buf()));
    let target = native::TargetSpec::x86_64_unknown_linux_gnu();
    let policy = ContentHash::of_bytes(b"cold operation tasks");
    let mut previous = BTreeMap::new();
    for (iteration, (literal, heap, fresh)) in [
        (7, false, false),
        (7, false, true),
        (8, false, false),
        (8, true, false),
        (8, true, true),
    ]
    .into_iter()
    .enumerate()
    {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine
            .sources
            .add(
                "operation-tasks.hir",
                super::operation::fixture(literal, heap),
            )
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        if fresh {
            engine.adapters.lowering = None;
        }
        let mut source_queries = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
        let declarations =
            NativeDeclarations::collect(&mut engine, &mut source_queries, &[]).unwrap();
        let source =
            NativeInstanceInputs::complete(&mut engine, declarations, store.clone()).unwrap();
        drop(engine);
        let trace = hirsel_lir::construction::ValueTrace::default();
        let _guard = trace.enter();
        let budget = ProcessBudget::new(768 << 20);
        let weight = MemoryWeight {
            scratch: 32 << 20,
            retained: 1 << 20,
            stack: 0,
        };
        let instances = InstanceTasks::new(
            source,
            store.clone(),
            native::Options::new(target.clone()),
            policy,
            weight,
            budget.reserve(32 << 20).unwrap(),
        )
        .unwrap();
        assert!(
            OperationTasks::new(
                instances.clone(),
                store.clone(),
                MemoryWeight {
                    retained: 0,
                    ..weight
                },
                budget.reserve(1).unwrap()
            )
            .is_err()
        );
        assert_eq!(budget.reserved(), 32 << 20);
        let operations = OperationTasks::new(
            instances.clone(),
            store.clone(),
            weight,
            budget.reserve(32 << 20).unwrap(),
        )
        .unwrap();
        let factory = Arc::new(OperationTaskFactory::new(operations.clone()));
        let mut frame = factory.create(operations.query_id()).unwrap();
        assert!(
            matches!(frame.poll(&mut Pending),QueryPoll::Waiting(ref reads) if reads==&vec![instances.discovery_id()])
        );
        assert_eq!(operations.evaluations.load(Ordering::Relaxed), 0);
        assert_eq!(instances.imports.load(Ordering::Relaxed), 0);
        assert_eq!(trace.totals(), Default::default());
        let denied = instances.failure("source refused");
        assert!(
            matches!(frame.poll(&mut Failed(denied)),QueryPoll::Failed(error) if error==denied)
        );
        drop(frame);
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
        let instance_factory = Arc::new(Traced(
            Arc::new(InstanceTaskFactory::new(instances.clone())),
            trace.clone(),
        ));
        let operation_factory = Arc::new(Traced(factory, trace.clone()));
        for kind in [INSTANCE_KIND, INSTANCE_INPUT_KIND, DISCOVERY_KIND] {
            runtime.register(kind, instance_factory.clone()).unwrap();
        }
        for kind in [OPERATION_GRAPH_KIND, OPERATION_COMPONENT_KIND] {
            runtime.register(kind, operation_factory.clone()).unwrap();
        }
        let events = Arc::new(Events::default());
        runtime.observe(events.clone()).unwrap();
        let request = runtime.request(&[operations.query_id(), operations.query_id()]);
        let selected = runtime.wait(request).unwrap_or_else(|error| {
            use store::ResultStore;
            panic!(
                "{}",
                String::from_utf8_lossy(&store.get_content(error).unwrap())
            )
        });
        assert_eq!(selected[0], selected[1]);
        let graph = operations.graph(selected[0]).unwrap();
        let request = runtime.request(&[instances.discovery_id()]);
        let discovery = runtime.wait(request).unwrap()[0];
        let completed = instances
            .completed_instances(discovery)
            .unwrap()
            .into_iter()
            .map(|(owner, output)| {
                (
                    owner.instance.definition().0.name.to_string(),
                    (owner, output),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            instances.imports.load(Ordering::Relaxed),
            if fresh {
                0
            } else if iteration == 0 {
                completed.len() as u64
            } else {
                1
            }
        );
        for name in ["first", "second", "main"] {
            assert_eq!(graph.get(&completed[name].0), Some(!heap));
            let component = graph
                .components
                .iter()
                .find(|c| c.members.contains(&completed[name].0))
                .unwrap();
            if let Some(old) = previous.insert(name, component.product) {
                if fresh || !heap {
                    assert_eq!(component.product, old);
                } else {
                    assert_ne!(component.product.record, old.record);
                }
            }
        }
        assert_eq!(
            operations.publications.load(Ordering::Relaxed),
            if iteration == 0 {
                graph.components.len() as u64
            } else if !fresh && heap {
                2
            } else {
                0
            }
        );
        assert_eq!(
            operations.evaluations.load(Ordering::Relaxed),
            graph.components.len() as u64
        );
        if fresh {
            assert_eq!(trace.totals(), Default::default());
            assert_eq!(instances.source_body_demands.load(Ordering::Relaxed), 0);
        }
        let main_component = graph
            .components
            .iter()
            .find(|component| component.members.contains(&completed["main"].0))
            .unwrap();
        let mut component_frame = OperationTaskFactory::new(operations.clone())
            .create(QueryId {
                kind: OPERATION_COMPONENT_KIND,
                address: main_component.product.address,
            })
            .unwrap();
        let evaluations = operations.evaluations.load(Ordering::Relaxed);
        assert!(
            matches!(component_frame.poll(&mut Pending), QueryPoll::Waiting(ref keys) if keys.len() == 2)
        );
        assert_eq!(operations.evaluations.load(Ordering::Relaxed), evaluations);
        assert!(
            matches!(component_frame.poll(&mut Failed(denied)), QueryPoll::Failed(error) if error == denied)
        );
        drop(component_frame);
        let log = events.0.lock().unwrap();
        for (from, to) in [
            (OPERATION_GRAPH_KIND, DISCOVERY_KIND),
            (OPERATION_COMPONENT_KIND, INSTANCE_KIND),
            (OPERATION_COMPONENT_KIND, OPERATION_COMPONENT_KIND),
            (OPERATION_GRAPH_KIND, OPERATION_COMPONENT_KIND),
        ] {
            assert!(log.iter().any(|event|matches!(event,RuntimeEvent::Read{query,dependency} if query.kind==from&&dependency.kind==to)),"missing actual edge {from:x}->{to:x}");
        }
        assert_eq!(
            log.iter()
                .filter(
                    |event| matches!(event,RuntimeEvent::Claim(id) if *id==operations.query_id())
                )
                .count(),
            1
        );
        let claims = log
            .iter()
            .filter_map(|event| match event {
                RuntimeEvent::Claim(id) if id.kind == OPERATION_COMPONENT_KIND => {
                    Some(id.address.0)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(claims.len(), graph.components.len());
        assert_eq!(claims.iter().collect::<BTreeSet<_>>().len(), claims.len());
        drop(log);
        let main = completed["main"].0.clone();
        drop(completed);
        drop(runtime);
        drop(operation_factory);
        drop(instance_factory);
        assert_eq!(budget.reserved(), 64 << 20);
        drop(operations);
        drop(instances);
        assert_eq!(
            budget.reserved(),
            32 << 20,
            "exported graph holds its actual descriptor reservation"
        );
        assert!(budget.reserve((736 << 20) + 1).is_err());
        assert_eq!(graph.get(&main), Some(!heap));
        drop(graph);
        assert_eq!(budget.reserved(), 0);
    }
}
