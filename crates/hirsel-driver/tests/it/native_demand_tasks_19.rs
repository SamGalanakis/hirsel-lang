use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::ResultStore, *};
use native::queries::{
    INSTANCE_KIND,
    declarations::NativeDeclarations,
    instance_tasks::{DISCOVERY_KIND, INSTANCE_INPUT_KIND, InstanceTaskFactory, InstanceTasks},
    source_inputs::NativeInstanceInputs,
};
use std::{
    sync::{Arc, atomic::Ordering},
    task::Poll,
};

struct Pending;
impl QueryContext for Pending {
    fn read(&mut self, _: QueryId) -> Poll<Result<ProductRef, store::Reference>> {
        Poll::Pending
    }
}
#[test]
fn native_demand_tasks_import_and_reuse_actual_settled_source_without_a_controller() {
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
            let inputs = NativeInstanceInputs::from_products(declarations, reader).unwrap();
            drop(discovery);
            drop(engine);
            assert!(originals.iter().all(|body| body.upgrade().is_none()));
            let budget = ProcessBudget::new(128 << 20);
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
                budget.reserve(112 << 20).unwrap(),
                errors,
            )
            .unwrap();
            for kind in [INSTANCE_KIND, INSTANCE_INPUT_KIND, DISCOVERY_KIND] {
                runtime.register(kind, factory.clone()).unwrap();
            }
            let request = runtime.request(&[tasks.discovery_id()]);
            let result = runtime.wait(request).unwrap_or_else(|error| {
                panic!(
                    "{}",
                    String::from_utf8_lossy(&store.get_content(error).unwrap())
                )
            });
            let completed = tasks.completed_instances(result[0]).unwrap();
            assert_eq!(completed.len(), 2);
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
                assert_eq!(tasks.imports.load(Ordering::Relaxed), 2);
                expected = Some(outputs);
            }
        }
    });
}
