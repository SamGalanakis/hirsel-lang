use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::ResultStore, *};
use native::queries::{
    ARENA_CANDIDATES_KIND, ARENA_CHOSEN_KIND, INSTANCE_KIND,
    arena_tasks::{ARENA_GRAPH_KIND, ArenaTaskFactory, ArenaTasks},
    instance::CALLABLE_VALUE_KIND,
    instance_tasks::{
        CALLABLE_GRAPH_KIND, DISCOVERY_KIND, INSTANCE_INPUT_KIND, InstanceTaskFactory,
        InstanceTasks,
    },
    source_inputs::NativeSourceProvider,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering},
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
#[derive(Debug, Eq, PartialEq)]
struct Observed {
    values: BTreeMap<String, store::Reference>,
    candidates: BTreeMap<String, (ProductRef, Vec<u32>)>,
    chosen: BTreeMap<String, (ProductRef, Vec<u32>)>,
}
fn selected(name: &str) -> bool {
    matches!(name, "first" | "second" | "wrapper")
}

fn observe(store: Arc<dyn ResultStore>, text: &str, fresh: bool, jobs: usize) -> Observed {
    let transport = ProcessBudget::new(24 << 20);
    let store: Arc<dyn ResultStore> = Arc::new(store::RequestStore::new(
        store,
        transport.reserve(24 << 20).unwrap(),
    ));
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
        .with_result_store(store.clone());
    let file = engine
        .sources
        .add("recursive-arena.hir", text.into())
        .unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    if fresh {
        engine.adapters.lowering = None;
    }
    let inputs = crate::native_front_fixture::inputs(&mut engine, &[]);
    let mut source = inputs.provider();
    let originals = inputs
        .declarations
        .context
        .sources
        .keys()
        .map(|path| {
            let body = source.original_source(path).unwrap().0;
            Arc::downgrade(&body)
        })
        .collect::<Vec<_>>();
    drop(source);
    drop(engine);
    assert!(originals.iter().all(|body| body.upgrade().is_none()));
    let runtime_bytes = hirsel_intern::jobs::COMPILER_STACK_BYTES as u64 + (64 << 20);
    let budget = ProcessBudget::new(runtime_bytes + (32 << 20));
    let trace = hirsel_lir::construction::ValueTrace::default();
    let _observe = trace.enter();
    let weight = MemoryWeight {
        scratch: 16 << 20,
        retained: 1 << 20,
        stack: 0,
    };
    // This witness measures ordinary production hits. EveryStage deliberately
    // reconstructs warm Values and has its own recomputation control.
    let mut options = native::Options::new(hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu());
    options.validation = native::Validation::Ready;
    let tasks = InstanceTasks::new(
        inputs,
        store.clone(),
        options,
        hirsel_syntax::ContentHash::of_bytes(b"recursive native arena witness"),
        weight,
        budget.reserve(16 << 20).unwrap(),
    )
    .unwrap();
    let errors = RuntimeErrors {
        resource: tasks.failure("recursive arena resource refusal"),
        cycle: tasks.failure("recursive arena dependency cycle refusal"),
        cancelled: tasks.failure("recursive arena cancellation"),
        protocol: tasks.failure("recursive arena protocol refusal"),
        panicked: tasks.failure("recursive arena worker panic"),
    };
    let mut runtime = AdmittedRuntime::new(
        hirsel_intern::jobs::Jobs::new(jobs),
        budget.reserve(runtime_bytes).unwrap(),
        errors,
    )
    .unwrap();
    let factory = Arc::new(Traced(
        Arc::new(InstanceTaskFactory::new(tasks.clone())),
        trace.clone(),
    ));
    for kind in [
        INSTANCE_KIND,
        INSTANCE_INPUT_KIND,
        DISCOVERY_KIND,
        CALLABLE_GRAPH_KIND,
        CALLABLE_VALUE_KIND,
    ] {
        runtime.register(kind, factory.clone()).unwrap();
    }
    let arenas = ArenaTasks::new(
        tasks.clone(),
        store.clone(),
        0,
        weight,
        budget.reserve(16 << 20).unwrap(),
    )
    .unwrap();
    let factory = Arc::new(Traced(
        Arc::new(ArenaTaskFactory::new(arenas.clone())),
        trace.clone(),
    ));
    for kind in [ARENA_CANDIDATES_KIND, ARENA_CHOSEN_KIND, ARENA_GRAPH_KIND] {
        runtime.register(kind, factory.clone()).unwrap();
    }
    assert_eq!(trace.totals(), Default::default());
    assert_eq!(tasks.imports.load(Ordering::Relaxed), 0);
    let request = runtime.request(&[tasks.discovery_id(), arenas.query_id()]);
    let outputs = runtime.wait(request).unwrap_or_else(|error| {
        panic!(
            "recursive arena: {:?}: {}",
            runtime.stats(),
            String::from_utf8_lossy(&store.get_content(error).unwrap()),
        )
    });
    let graph = arenas.graph((outputs[1]).clone()).unwrap();
    for summaries in [&graph.candidates, &graph.chosen] {
        assert!(
            summaries.iter().any(|summary| {
                ["first", "second"].into_iter().all(|name| {
                    summary
                        .candidates
                        .iter()
                        .any(|(key, _)| key.instance.definition().0.name == name)
                })
            }),
            "the actual two-node SCC must remain one component in each stratum"
        );
    }
    let rows = |summaries: &[Arc<native::queries::ArenaSummary>]| {
        summaries
            .iter()
            .flat_map(|summary| {
                summary.candidates.iter().filter_map(|(owner, claims)| {
                    let name = &owner.instance.definition().0.name;
                    selected(name)
                        .then(|| (name.clone(), (summary.product.clone(), claims.clone())))
                })
            })
            .collect::<BTreeMap<_, _>>()
    };
    let values = tasks
        .completed_instances((outputs[0]).clone())
        .unwrap()
        .iter()
        .filter_map(|(owner, output)| {
            let name = &owner.instance.definition().0.name;
            selected(name).then(|| (name.clone(), output.product.value))
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(values.len(), 3);
    if fresh {
        assert_eq!(tasks.imports.load(Ordering::Relaxed), 0);
        assert_eq!(tasks.source_body_demands.load(Ordering::Relaxed), 0);
        assert_eq!(arenas.solves.load(Ordering::Relaxed), 0);
        assert_eq!(trace.totals(), Default::default());
    }
    Observed {
        values,
        candidates: rows(&graph.candidates),
        chosen: rows(&graph.chosen),
    }
}

#[test]
fn native_arena_recursive_source_edit_propagates_through_both_strata_without_reimporting_callers() {
    // The exact mandatory ce-native-recipes-16 before/after witness.
    let before = include_str!("../fixtures/native-arena-19/recursive-before.hir");
    let after = include_str!("../fixtures/native-arena-19/recursive-after.hir");
    with_compiler_stack(|| {
        let mut width_one = None;
        for jobs in [1, 2] {
            let directory = tempfile::tempdir().unwrap();
            let store: Arc<dyn ResultStore> =
                Arc::new(store::DirStore::new(directory.path().to_path_buf()));
            let initial = observe(store.clone(), before, false, jobs);
            assert_eq!(initial, observe(store.clone(), before, true, jobs));
            for name in ["first", "second", "wrapper"] {
                assert_eq!(initial.candidates[name].1, [0], "{name}");
                assert_eq!(initial.chosen[name].1, [0], "{name}");
            }
            let edited = observe(store.clone(), after, false, jobs);
            assert_eq!(edited, observe(store, after, true, jobs));
            assert_ne!(initial.values["first"], edited.values["first"]);
            for name in ["first", "second", "wrapper"] {
                assert!(edited.candidates[name].1.is_empty(), "{name}");
                assert!(edited.chosen[name].1.is_empty(), "{name}");
                assert_ne!(
                    initial.chosen[name]
                        .0
                        .archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                        .unwrap()
                        .reference(),
                    edited.chosen[name]
                        .0
                        .archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                        .unwrap()
                        .reference(),
                    "{name}"
                );
            }
            for name in ["second", "wrapper"] {
                assert_eq!(
                    initial.values[name], edited.values[name],
                    "{name} base Value must backdate"
                );
            }
            if let Some(expected) = &width_one {
                assert_eq!(
                    expected,
                    &(initial, edited),
                    "worker width must preserve semantic products"
                );
            } else {
                width_one = Some((initial, edited));
            }
        }
    });
}
