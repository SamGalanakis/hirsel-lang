use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::ResultStore, *};
use native::queries::{
    ARENA_CANDIDATES_KIND, ARENA_CHOSEN_KIND, INSTANCE_KIND, READY_KIND,
    arena_tasks::{ARENA_GRAPH_KIND, ArenaTaskFactory, ArenaTasks},
    declarations::NativeDeclarations,
    instance_tasks::{DISCOVERY_KIND, INSTANCE_INPUT_KIND, InstanceTaskFactory, InstanceTasks},
    products::{EmissionOptions, NativeSnapshot},
    source_inputs::NativeInstanceInputs,
    tasks::{NativeTaskFactory, NativeTasks},
};
use std::{
    sync::{Arc, atomic::Ordering},
    task::Poll,
};

struct Traced(Arc<dyn QueryFactory>, hirsel_lir::construction::ValueTrace);
struct TracedTask(Box<dyn QueryTask>, hirsel_lir::construction::ValueTrace);
impl QueryFactory for Traced {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, store::Reference> {
        Ok(Box::new(TracedTask(self.0.create(id)?, self.1.clone())))
    }
}
impl QueryTask for TracedTask {
    fn id(&self) -> QueryId {
        self.0.id()
    }
    fn weight(&self) -> MemoryWeight {
        self.0.weight()
    }
    fn continuation_bytes(&self) -> u64 {
        self.0.continuation_bytes()
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        let _observe = self.1.enter();
        self.0.poll(cx)
    }
}
struct Pending;
impl QueryContext for Pending {
    fn read(&mut self, _: QueryId) -> Poll<Result<ProductRef, store::Reference>> {
        Poll::Pending
    }
}
#[test]
fn native_ready_tasks_validate_cold_and_fresh_snapshots_with_current_source_invocations() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn ResultStore> =
            Arc::new(store::DirStore::new(dir.path().join("products")));
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let policy =
            hirsel_syntax::ContentHash::of_bytes(b"native demand task current source control");
        let mut expected = None;
        for fresh in [false, true] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine
                .sources
                .add(
                    "settled-demand.hir",
                    "fn other()->u64{9u64}\npub fn main()->u64{7u64}".into(),
                )
                .unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            if fresh {
                engine.adapters.lowering = None;
            }
            let mut discovery = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
            let declarations =
                NativeDeclarations::collect(&mut engine, &mut discovery, &[]).unwrap();
            let paths = declarations
                .context
                .sources
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            let context = engine.native_original_context(&paths).unwrap();
            let records = NativeRecords::new(engine.types().clone(), store.clone());
            let mut products = Vec::new();
            let mut originals = Vec::new();
            for path in &paths {
                let source = &declarations.sources[path];
                let (body, verified) = engine.native_definition_body_with_output(source).unwrap();
                originals.push(Arc::downgrade(&body));
                let bytes = records.encode(&engine, &body).unwrap();
                products.push(NativeOriginalProduct {
                    definition: path.clone(),
                    implementation_revision: source.source.implementation_revision,
                    signature: source.signature.fingerprint,
                    body: store.put_content(NATIVE_ORIGINAL_BODY, &bytes).unwrap(),
                    verified,
                });
            }
            let reader = context.reader(products, store.clone()).unwrap();
            let manifest = declarations.manifest().unwrap();
            let input = declarations.program.input.clone();
            let inputs = NativeInstanceInputs::from_products(declarations, reader).unwrap();
            drop(discovery);
            drop(engine);
            assert!(originals.iter().all(|body| body.upgrade().is_none()));
            let runtime_bytes = hirsel_intern::jobs::COMPILER_STACK_BYTES as u64 + (64 << 20);
            let budget = ProcessBudget::new(runtime_bytes + (32 << 20));
            let trace = hirsel_lir::construction::ValueTrace::default();
            let _observe = trace.enter();
            let tasks = InstanceTasks::new(
                inputs,
                store.clone(),
                native::Options::new(target.clone()),
                policy,
                MemoryWeight {
                    scratch: 16 << 20,
                    retained: 1 << 20,
                    stack: 0,
                },
                budget.reserve(16 << 20).unwrap(),
            )
            .unwrap();
            let factory = Arc::new(InstanceTaskFactory::new(tasks.clone()));
            let main = tasks
                .roots()
                .iter()
                .find(|key| key.instance.definition().0.name == "main")
                .unwrap();
            let id = QueryId {
                kind: INSTANCE_KIND,
                address: main.address(INSTANCE_KIND),
            };
            let mut task = factory.create(id).unwrap();
            assert!(matches!(task.poll(&mut Pending), QueryPoll::Waiting(_)));
            assert_eq!(trace.totals(), Default::default());
            assert_eq!(tasks.imports.load(Ordering::Relaxed), 0);
            let denied = tasks.failure("native task control refusal");
            let errors = RuntimeErrors {
                resource: denied,
                cycle: denied,
                cancelled: denied,
                protocol: denied,
                panicked: denied,
            };
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(1),
                budget.reserve(runtime_bytes).unwrap(),
                errors,
            )
            .unwrap();
            let traced = Arc::new(Traced(factory.clone(), trace.clone()));
            for kind in [INSTANCE_KIND, INSTANCE_INPUT_KIND, DISCOVERY_KIND] {
                runtime.register(kind, traced.clone()).unwrap();
            }
            let request = runtime.request(&[tasks.discovery_id()]);
            let result = runtime.wait(request).unwrap_or_else(|error| {
                panic!(
                    "{}",
                    String::from_utf8_lossy(&store.get_content(error).unwrap())
                )
            });
            let discovery_output = result[0];
            let arenas = ArenaTasks::new(
                tasks.clone(),
                store.clone(),
                0,
                MemoryWeight {
                    scratch: 16 << 20,
                    retained: 1 << 20,
                    stack: 0,
                },
                budget.reserve(16 << 20).unwrap(),
            )
            .unwrap();
            let arena_factory = Arc::new(Traced(
                Arc::new(ArenaTaskFactory::new(arenas.clone())),
                trace.clone(),
            ));
            for kind in [ARENA_CANDIDATES_KIND, ARENA_CHOSEN_KIND, ARENA_GRAPH_KIND] {
                runtime.register(kind, arena_factory.clone()).unwrap();
            }
            let arena_request = runtime.request(&[arenas.query_id()]);
            let arena_output = runtime.wait(arena_request).unwrap_or_else(|error| {
                panic!(
                    "{}",
                    String::from_utf8_lossy(&store.get_content(error).unwrap())
                )
            })[0];
            let graph = arenas.graph(arena_output).unwrap();
            assert!(!graph.candidates.is_empty() && !graph.chosen.is_empty());
            let operations = Arc::new(tasks.source_operations(discovery_output).unwrap());
            assert_eq!(
                operations
                    .invocation_bodies
                    .iter()
                    .filter(
                        |row| row.definition.0.name == "main" || row.definition.0.name == "other"
                    )
                    .count(),
                2
            );
            let products = tasks.products(discovery_output).unwrap();
            let snapshot = NativeSnapshot::new(
                store.clone(),
                products,
                graph,
                native::Options::new(target.clone()),
                EmissionOptions::request(
                    input,
                    hirsel_lir::emission::OptLevel::None,
                    false,
                    Default::default(),
                    true,
                    false,
                    false,
                ),
                manifest,
                operations,
            )
            .unwrap();
            drop(runtime);
            let ready_tasks = NativeTasks::new(
                snapshot.clone(),
                MemoryWeight {
                    scratch: 16 << 20,
                    retained: 1 << 20,
                    stack: 0,
                },
            )
            .unwrap();
            let ready_factory = Arc::new(Traced(
                Arc::new(NativeTaskFactory::new(ready_tasks.clone())),
                trace.clone(),
            ));
            let mut ready_runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(1),
                budget.reserve(runtime_bytes).unwrap(),
                errors,
            )
            .unwrap();
            for kind in [
                INSTANCE_KIND,
                ARENA_CANDIDATES_KIND,
                ARENA_CHOSEN_KIND,
                READY_KIND,
            ] {
                ready_runtime.register(kind, ready_factory.clone()).unwrap();
            }
            let selected = ready_runtime.request(&[main.ready_query_id()]);
            let ready = ready_runtime.wait(selected).unwrap_or_else(|error| {
                panic!(
                    "{}",
                    String::from_utf8_lossy(&store.get_content(error).unwrap())
                )
            });
            assert_eq!(ready.len(), 1);
            assert_eq!(
                ready_tasks.ready_transformations.load(Ordering::Relaxed),
                u64::from(!fresh)
            );
            assert_eq!(snapshot.ready_materializations(), 0);
            let completed = tasks.completed_instances(result[0]).unwrap();
            assert_eq!(
                completed
                    .iter()
                    .filter(|(key, _)| key.instance.definition().0.name == "main"
                        || key.instance.definition().0.name == "other")
                    .count(),
                2
            );
            let outputs = completed
                .iter()
                .map(|(_, output)| output.product.product)
                .collect::<Vec<_>>();
            if fresh {
                assert_eq!(expected.as_ref(), Some(&outputs));
                assert_eq!(tasks.imports.load(Ordering::Relaxed), 0);
                assert_eq!(tasks.source_body_demands.load(Ordering::Relaxed), 0);
                assert_eq!(trace.totals(), Default::default());
            } else {
                assert_eq!(
                    tasks.imports.load(Ordering::Relaxed),
                    completed.len() as u64
                );
                expected = Some(outputs);
            }
        }
    });
}
