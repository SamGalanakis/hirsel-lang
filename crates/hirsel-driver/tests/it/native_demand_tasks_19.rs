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
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            if fresh {
                engine.adapters.lowering = None;
            }
            let facts = crate::native_front_fixture::products(&mut engine, &[]).unwrap();
            let declarations = NativeDeclarations::from_front(facts.clone()).unwrap();
            let reader = facts
                .original_context
                .reader(
                    facts.originals.iter().map(|product| (**product).clone()),
                    store.clone(),
                )
                .unwrap();
            let originals = facts
                .originals
                .iter()
                .map(|product| {
                    Arc::downgrade(reader.read(&product.definition).unwrap().definition())
                })
                .collect::<Vec<_>>();
            drop(facts);
            let inputs = NativeInstanceInputs::from_products(declarations, reader).unwrap();
            drop(engine);
            assert!(originals.iter().all(|body| body.upgrade().is_none()));
            let runtime_bytes = hirsel_intern::jobs::COMPILER_STACK_BYTES as u64 + (64 << 20);
            let budget = ProcessBudget::new(runtime_bytes + (16 << 20));
            let trace = hirsel_lir::construction::ValueTrace::default();
            let _observe = trace.enter();
            // This control exercises production hit admission; EveryStage
            // intentionally recomputes instead of consuming stored Values.
            let mut options = native::Options::new(target.clone());
            options.validation = native::Validation::Ready;
            let tasks = InstanceTasks::new(
                inputs,
                store.clone(),
                options,
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
            for kind in [
                INSTANCE_KIND,
                INSTANCE_INPUT_KIND,
                DISCOVERY_KIND,
                hirsel_driver::native::queries::instance_tasks::CALLABLE_GRAPH_KIND,
                hirsel_driver::native::queries::instance::CALLABLE_VALUE_KIND,
            ] {
                runtime.register(kind, traced.clone()).unwrap();
            }
            let request = runtime.request(&[tasks.discovery_id()]);
            let result = runtime.wait(request).unwrap_or_else(|error| {
                panic!(
                    "{}",
                    String::from_utf8_lossy(&store.get_content(error).unwrap())
                )
            });
            let completed = tasks.completed_instances(result[0]).unwrap();
            // The same compact flow archive survives cold publication and a
            // fresh hit with lowering disabled. This observes no Value body.
            for (key, output) in &completed {
                output.facts.body.seal_demand.validate().unwrap();
                assert_eq!(
                    output.facts.body.seal_demand.owner(),
                    &(key.instance.definition().clone(), key.instance.digest())
                );
                let encoded = serde_json::to_vec(&output.facts.body.seal_demand).unwrap();
                let restored: hirsel_rc::seal_demand::equations::LocalDemandEquations =
                    serde_json::from_slice(&encoded).unwrap();
                assert_eq!(restored, output.facts.body.seal_demand);
                let mut malformed: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
                malformed["seals"] = serde_json::json!([u32::MAX]);
                let malformed: hirsel_rc::seal_demand::equations::LocalDemandEquations =
                    serde_json::from_value(malformed).unwrap();
                assert!(malformed.validate().is_err());
            }
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

#[test]
fn native_source_metadata_debits_one_grant_and_outlives_the_instance_registry() {
    with_compiler_stack(|| {
        let store: Arc<dyn ResultStore> = Arc::new(store::MemStore::default());
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine
            .sources
            .add("metadata-admission.hir", "pub fn main()->u64{1u64}".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let facts = crate::native_front_fixture::products(&mut engine, &[]).unwrap();
        let declarations = NativeDeclarations::from_front(facts.clone()).unwrap();
        let reader = facts
            .original_context
            .reader(std::iter::empty(), store.clone())
            .unwrap();
        drop(facts);
        let inputs = NativeInstanceInputs::from_products(declarations, reader).unwrap();
        drop(engine);
        let copied = inputs.retained_metadata_bytes();
        assert!(copied > 0);
        let budget = ProcessBudget::new(16 << 20);
        let make = |grant| {
            InstanceTasks::new(
                inputs.clone(),
                store.clone(),
                native::Options::new(hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu()),
                hirsel_syntax::ContentHash::of_bytes(b"metadata-admission"),
                MemoryWeight {
                    scratch: 16 << 20,
                    retained: 1 << 20,
                    stack: 0,
                },
                grant,
            )
        };
        let error = match make(budget.reserve(copied - 1).unwrap()) {
            Ok(_) => panic!("insufficient copied metadata grant was admitted"),
            Err(error) => error,
        };
        assert_eq!(error, "native source metadata exceeds retained allowance");
        assert_eq!(budget.reserved(), 0);
        let tasks = make(budget.reserve(16 << 20).unwrap()).unwrap();
        let retained = tasks.retained_source_allowance();
        assert_eq!(retained.bytes(), copied);
        assert_eq!(budget.reserved(), 16 << 20);
        drop(tasks);
        assert_eq!(budget.reserved(), copied);
        // A second registry uses the already held input debit without a second
        // split; its independent registry portion still fits the original cap.
        let tasks = make(budget.reserve((16 << 20) - copied).unwrap()).unwrap();
        assert!(Arc::ptr_eq(&retained, &tasks.retained_source_allowance()));
        drop(tasks);
        assert_eq!(budget.reserved(), copied);
        drop(inputs);
        assert_eq!(budget.reserved(), copied);
        drop(retained);
        assert_eq!(budget.reserved(), 0);
    });
}
