use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::ResultStore, *};
use native::queries::{
    ARENA_CANDIDATES_KIND, ARENA_CHOSEN_KIND, INSTANCE_KIND, READY_KIND,
    arena_tasks::{ARENA_GRAPH_KIND, ArenaTaskFactory, ArenaTasks},
    declarations::NativeDeclarations,
    instance::CALLABLE_VALUE_KIND,
    instance_tasks::{
        CALLABLE_GRAPH_KIND, DISCOVERY_KIND, INSTANCE_INPUT_KIND, InstanceTaskFactory,
        InstanceTasks,
    },
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
    ready_case(
        "fn other()->u64{9u64}\npub fn main()->u64{7u64}",
        false,
        false,
        false,
    );
}
#[test]
fn native_ready_tasks_reuse_specialized_callable_dependencies_without_materialization() {
    ready_case_inner_mutation(
        "fn other()->u64{9u64}\nfn apply(f:fn()->u64 uses {})->u64 uses {f}{f()}\npub fn main()->u64{apply(other)}",
        false,
        false,
        false,
        native::Validation::Ready,
        store::VerifyStore::Off,
        0,
        0,
        1,
    );
}
const ADAPTER_SOURCE: &str = r#"
fn other(x:u64)->u64 { 10u64/x }
pub fn main()->Result[(),()] {
 let mut s:Set[String]=set.empty()
 let insert:fn(mut Set[String],take String)->()=set.insert
 let _=insert(mut s,"key")
 let f=other
 let _=f(2u64)
 Ok(())
}"#;
#[test]
fn native_ready_tasks_preserve_actual_materialized_adapter_inventory_on_cold_and_fresh_reads() {
    ready_case(ADAPTER_SOURCE, true, false, false);
}
#[test]
fn native_ready_tasks_refuse_missing_independent_adapter_inventory_before_materialization() {
    ready_case(ADAPTER_SOURCE, true, true, false);
}
#[test]
fn every_stage_recomputes_instance_and_ready_hits_in_fresh_snapshots() {
    for validation in [native::Validation::Ready, native::Validation::EveryStage] {
        ready_case_policy(
            "fn other()->u64{9u64}\npub fn main()->u64{7u64}",
            false,
            false,
            false,
            validation,
            store::VerifyStore::Off,
        );
    }
}

#[test]
fn explicit_store_verification_recomputes_instance_hits_under_boundary_validation() {
    ready_case_policy(
        "fn other()->u64{9u64}\npub fn main()->u64{7u64}",
        false,
        false,
        false,
        native::Validation::Ready,
        store::VerifyStore::Recompute,
    );
}

pub(super) fn ready_case(source: &str, adapter: bool, missing_inventory: bool, assemble: bool) {
    ready_case_policy(
        source,
        adapter,
        missing_inventory,
        assemble,
        native::Validation::Ready,
        store::VerifyStore::Off,
    );
}

#[test]
fn ownership_ready_query_keeps_value_identities_and_refuses_stale_dependencies() {
    ready_case_inner(
        "resource type Token { value:u64 }\ndrop(x) {}\nfn other()->u64{9u64}\npub fn main()->u64{let first=Token(value:7u64)\nlet second=take first\n7u64}",
        false,
        false,
        false,
        native::Validation::Ready,
        store::VerifyStore::Off,
        1,
    );
}

#[test]
fn ownership_query_root_waits_for_dependencies_and_revalidates_selected_outputs() {
    ready_case_inner(
        "resource type Token { value:u64 }\ndrop(x) {}\nfn other()->u64{9u64}\npub fn main()->u64{let first=Token(value:7u64)\nlet second=take first\n7u64}",
        false,
        false,
        false,
        native::Validation::Ready,
        store::VerifyStore::Off,
        2,
    );
}

#[test]
fn ownership_compilation_root_discovers_source_and_arena_before_diagnostics() {
    ready_case_inner(
        "resource type Token { value:u64 }\ndrop(x) {}\nfn other()->u64{9u64}\npub fn main()->u64{let first=Token(value:7u64)\nlet second=take first\n7u64}",
        false,
        false,
        false,
        native::Validation::Ready,
        store::VerifyStore::Off,
        3,
    );
}

#[test]
fn ownership_graph_binding_emits_original_value_plans_on_both_backends() {
    ready_case_inner(
        "resource type Token { value:u64 }\ndrop(x) {}\nfn other()->u64{9u64}\npub fn main()->u64{let first=Token(value:7u64)\nlet second=take first\n7u64}",
        false,
        false,
        false,
        native::Validation::Ready,
        store::VerifyStore::Off,
        4,
    );
}

fn ready_case_policy(
    source: &str,
    adapter: bool,
    missing_inventory: bool,
    assemble: bool,
    validation: native::Validation,
    verify_store: store::VerifyStore,
) {
    ready_case_inner(
        source,
        adapter,
        missing_inventory,
        assemble,
        validation,
        verify_store,
        0,
    );
}

#[allow(clippy::too_many_arguments)]
fn ready_case_inner(
    source: &str,
    adapter: bool,
    missing_inventory: bool,
    assemble: bool,
    validation: native::Validation,
    verify_store: store::VerifyStore,
    ownership: u8,
) {
    ready_case_inner_mutation(
        source,
        adapter,
        missing_inventory,
        assemble,
        validation,
        verify_store,
        ownership,
        u8::from(missing_inventory),
        0,
    );
}

#[allow(clippy::too_many_arguments)]
fn ready_case_inner_mutation(
    source: &str,
    adapter: bool,
    missing_inventory: bool,
    assemble: bool,
    validation: native::Validation,
    verify_store: store::VerifyStore,
    ownership: u8,
    mutation: u8,
    expected_specializations: usize,
) {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let transport = ProcessBudget::new(64 << 20);
        let store: Arc<dyn ResultStore> = Arc::new(store::RequestStore::new(
            Arc::new(store::DirStore::new(dir.path().join("products"))),
            transport.reserve(64 << 20).unwrap(),
        ));
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let policy =
            hirsel_syntax::ContentHash::of_bytes(b"native demand task current source control");
        let mut options = native::Options::new(target.clone());
        options.validation = validation;
        let recompute = validation == native::Validation::EveryStage;
        let recompute_instances = recompute || verify_store == store::VerifyStore::Recompute;
        let mut expected = None;
        for fresh in [false, true] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine
                .sources
                .add("settled-demand.hir", source.into())
                .unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            if fresh {
                engine.adapters.lowering = None;
            }
            let facts = crate::native_front_fixture::products(&mut engine, &[]).unwrap();
            let declarations = NativeDeclarations::from_front(facts.clone()).unwrap();
            let reader = facts
                .original_context
                .reader(facts.originals.iter().map(|product| (**product).clone()))
                .unwrap();
            let originals = facts
                .originals
                .iter()
                .map(|product| {
                    Arc::downgrade(reader.read(&product.definition).unwrap().definition())
                })
                .collect::<Vec<_>>();
            drop(facts);
            let manifest = declarations.manifest().unwrap();
            let input = declarations.program.input.clone();
            let inputs = NativeInstanceInputs::from_products(declarations, reader).unwrap();
            drop(engine);
            assert!(originals.iter().all(|body| body.upgrade().is_none()));
            let runtime_bytes = hirsel_intern::jobs::COMPILER_STACK_BYTES as u64 + (64 << 20);
            let budget = ProcessBudget::new(runtime_bytes + (32 << 20));
            let trace = hirsel_lir::construction::ValueTrace::default();
            let _observe = trace.enter();
            let mut instance_allowance = budget.reserve(16 << 20).unwrap();
            let mut diagnostic_allowance = if ownership >= 2 {
                Some(instance_allowance.split(2 << 20).unwrap())
            } else {
                None
            };
            let tasks = InstanceTasks::new(
                inputs,
                store.clone(),
                options.clone(),
                policy,
                MemoryWeight {
                    scratch: 16 << 20,
                    retained: 1 << 20,
                    stack: 0,
                },
                instance_allowance,
            )
            .unwrap()
            .with_store_verification(verify_store)
            .unwrap();
            if verify_store == store::VerifyStore::Recompute {
                let shared = tasks.clone();
                let error = match shared.with_store_verification(store::VerifyStore::Off) {
                    Err(error) => error,
                    Ok(_) => panic!("shared native Instance policy was mutated"),
                };
                assert!(
                    error.contains("shared before store verification binding"),
                    "{error}"
                );
            }
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
                resource: tasks.failure("runtime resource admission refused"),
                cycle: tasks.failure("runtime dependency cycle refused"),
                cancelled: tasks.failure("runtime request cancelled"),
                protocol: tasks.failure("runtime dependency protocol refused"),
                panicked: tasks.failure("runtime worker panicked"),
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
                CALLABLE_GRAPH_KIND,
                CALLABLE_VALUE_KIND,
            ] {
                runtime.register(kind, traced.clone()).unwrap();
            }
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
            if ownership == 3 {
                use native::queries::ownership_compilation::OwnershipCompilation;
                use native::queries::ownership_tasks::{OWNERSHIP_READY_KIND, OWNERSHIP_ROOT_KIND};
                let compilation = OwnershipCompilation::new(
                    tasks.clone(),
                    arenas.clone(),
                    EmissionOptions::request(
                        input.clone(),
                        hirsel_lir::emission::OptLevel::None,
                        false,
                        Default::default(),
                        true,
                        false,
                        false,
                    ),
                    MemoryWeight {
                        scratch: 16 << 20,
                        retained: 1 << 20,
                        stack: 0,
                    },
                    diagnostic_allowance.take().unwrap(),
                    verify_store,
                )
                .unwrap();
                let factories = compilation.factories();
                assert_eq!(tasks.imports.load(Ordering::Relaxed), 0);
                for (kind, factory) in factories {
                    if kind == native::queries::ownership_compilation::OWNERSHIP_COMPILATION_KIND
                        || kind == OWNERSHIP_READY_KIND
                        || kind == OWNERSHIP_ROOT_KIND
                    {
                        runtime.register(kind, factory).unwrap();
                    }
                }
                let selected = runtime.request(&[compilation.query_id()]);
                let completed = runtime.wait(selected).unwrap_or_else(|error| {
                    panic!(
                        "ownership compilation: {}",
                        String::from_utf8_lossy(&store.get_content(error).unwrap())
                    )
                })[0]
                    .clone();
                let (roots, root) = compilation.outcome(completed.clone()).unwrap();
                let outputs = roots.outputs(root).unwrap();
                assert!(outputs.len() >= 3);
                let mut resources = 0;
                for output in outputs {
                    let observed = roots.materialize(output).unwrap();
                    resources += observed
                        .initial
                        .observations
                        .iter()
                        .filter(|row| row.marker.kind == hirsel_lir::ownership_trace::Kind::Drop)
                        .count();
                }
                assert!(resources > 0, "actual source resource cleanup");
                let mut wrong = completed;
                wrong.action = store::Action(hirsel_syntax::ContentHash::of_bytes(
                    b"another ownership compilation",
                ));
                assert!(compilation.outcome(wrong).is_err());
                continue;
            }
            let request = runtime.request(&[tasks.discovery_id()]);
            let result = runtime.wait(request).unwrap_or_else(|error| {
                panic!(
                    "discovery phase: {error:?}, {:?}: {}",
                    runtime.stats(),
                    String::from_utf8_lossy(&store.get_content(error).unwrap())
                )
            });
            let discovery_output = result[0].clone();
            let arena_request = runtime.request(&[arenas.query_id()]);
            let arena_output = runtime.wait(arena_request).unwrap_or_else(|error| {
                panic!(
                    "arena phase: {error:?}, {:?}: {}",
                    runtime.stats(),
                    String::from_utf8_lossy(&store.get_content(error).unwrap())
                )
            })[0]
                .clone();
            let graph = arenas.graph(arena_output).unwrap();
            assert!(!graph.candidates.is_empty() && !graph.chosen.is_empty());
            let mut operations = tasks.source_operations(discovery_output.clone()).unwrap();
            assert_eq!(
                operations
                    .invocation_bodies
                    .iter()
                    .filter(
                        |row| row.definition.0.name == "main" || row.definition.0.name == "other"
                    )
                    .count(),
                if source.contains("fn other") { 2 } else { 1 }
            );
            if adapter {
                assert!(
                    !operations.pretransfer_adapters.is_empty(),
                    "completed source discovery must retain its real adapter inventory"
                );
                assert_eq!(
                    operations.pretransfer_actuals.len(),
                    operations.pretransfer_adapters.len(),
                    "canonical ABI actuals must come from the independent source planner"
                );
            }
            if missing_inventory {
                match mutation {
                    1 => operations.pretransfer_adapters.clear(),
                    2 => operations.pretransfer_adapters[0].abi = hirsel_lir::FnAbiId(u32::MAX),
                    3 => operations
                        .pretransfer_adapters
                        .push(operations.pretransfer_adapters[0].clone()),
                    4 => operations.invocation_bodies.clear(),
                    _ => panic!("unknown original keyed inventory mutation"),
                }
            }
            let products = tasks.products(discovery_output.clone()).unwrap();
            let before_snapshot = trace.totals();
            let snapshot = NativeSnapshot::new(
                store.clone(),
                products,
                graph,
                options.clone(),
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
                Arc::new(operations),
            );
            if missing_inventory {
                let Err(error) = snapshot else {
                    panic!("snapshot accepted independent inventory mutation {mutation}");
                };
                if mutation == 1 {
                    assert!(
                        error.contains("lost an independently planned adapter"),
                        "{error}"
                    );
                } else {
                    assert!(!error.is_empty(), "missing refusal diagnostic");
                }
                assert_eq!(
                    trace.totals(),
                    before_snapshot,
                    "missing inventory must fail before native materialization"
                );
                if fresh {
                    assert_eq!(tasks.imports.load(Ordering::Relaxed), 0);
                    assert_eq!(tasks.source_body_demands.load(Ordering::Relaxed), 0);
                    assert_eq!(trace.totals(), Default::default());
                }
                continue;
            }
            let snapshot = snapshot.unwrap();
            if ownership == 1 {
                use hirsel_lir::ownership_trace::{Kind, native::LeafPlan};
                use native::queries::{NativeProducts, products::DemandProducts};
                let mut products = DemandProducts::new(snapshot.clone());
                // Settle an ordinary Ready hit first: it must never replace
                // the Value producer and every checked diagnostic recipe.
                let ordinary = products.ready(main).unwrap();
                let observed = products.observe(main).unwrap();
                assert!(!observed.initial.observations.is_empty());
                assert_eq!(observed.recipes.len(), 10);
                assert_eq!(observed.recipes[0].before, observed.initial.unit);
                assert_eq!(
                    observed.recipes.last().unwrap().after,
                    observed.unit.unit().digest()
                );
                for recipes in observed.recipes.windows(2) {
                    assert_eq!(recipes[0].after, recipes[1].before);
                }
                let markers = |plan: &hirsel_lir::ownership_trace::Plan| {
                    plan.observations
                        .iter()
                        .map(|row| (row.marker.id, row.marker.kind, row.marker.site))
                        .collect::<Vec<_>>()
                };
                assert_eq!(markers(&observed.initial), markers(&observed.plan));
                assert!(
                    observed
                        .initial
                        .observations
                        .iter()
                        .any(|row| row.marker.kind == Kind::Create)
                );
                assert!(
                    observed
                        .initial
                        .observations
                        .iter()
                        .any(|row| row.marker.kind == Kind::Move)
                );
                assert!(
                    observed
                        .initial
                        .observations
                        .iter()
                        .any(|row| row.marker.kind == Kind::Drop)
                );
                LeafPlan::from_relocated(&observed.unit, &observed.layouts, &observed.plan)
                    .unwrap();
                let mut dropped = observed.plan.clone();
                dropped.observations.pop();
                assert!(
                    LeafPlan::from_relocated(&observed.unit, &observed.layouts, &dropped).is_err()
                );
                // Diagnostic work must not replace the ordinary success association.
                assert_eq!(products.ready(main).unwrap().product, ordinary.product);
                let mut stale = main.clone();
                stale.policy = hirsel_syntax::ContentHash::of_bytes(b"stale diagnostic selection");
                assert!(products.observe(&stale).is_err());
                let mut forged = observed.plan.clone();
                forged.observations[0].marker.id = hirsel_lir::ownership_trace::MarkerId(999_999);
                assert!(
                    hirsel_lir::ownership_trace::check_transform(
                        observed.unit.unit(),
                        &observed.plan,
                        observed.unit.unit(),
                        &forged,
                        Some(&hirsel_lir::ownership_trace::Recipe {
                            cleanups: Vec::new(),
                            before: observed.plan.unit,
                            after: forged.unit,
                            relocations: Vec::new(),
                        }),
                    )
                    .is_err()
                );
                continue;
            }
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
                CALLABLE_VALUE_KIND,
                ARENA_CANDIDATES_KIND,
                ARENA_CHOSEN_KIND,
                READY_KIND,
            ] {
                ready_runtime.register(kind, ready_factory.clone()).unwrap();
            }
            if ownership == 2 || ownership == 4 {
                use native::queries::ownership_tasks::{
                    OWNERSHIP_READY_KIND, OWNERSHIP_ROOT_KIND, OwnershipTasks,
                };
                let roots = OwnershipTasks::new(
                    ready_tasks.clone(),
                    MemoryWeight {
                        scratch: 16 << 20,
                        retained: 1 << 20,
                        stack: 0,
                    },
                    diagnostic_allowance.take().unwrap(),
                )
                .unwrap();
                let factory = roots.factory();
                let mut pending = factory.create(roots.query_id()).unwrap();
                assert!(matches!(pending.poll(&mut Pending), QueryPoll::Waiting(_)));
                assert_eq!(roots.transformations.load(Ordering::Relaxed), 0);
                let mut invalid = roots.query_id();
                invalid.kind = READY_KIND;
                assert!(factory.create(invalid).is_err());
                for kind in [OWNERSHIP_READY_KIND, OWNERSHIP_ROOT_KIND] {
                    ready_runtime.register(kind, factory.clone()).unwrap();
                }
                let selected = ready_runtime.request(&[roots.query_id()]);
                let root = ready_runtime.wait(selected).unwrap_or_else(|error| {
                    panic!(
                        "ownership root: {}",
                        String::from_utf8_lossy(&store.get_content(error).unwrap())
                    )
                })[0]
                    .clone();
                let outputs = roots.outputs(root.clone()).unwrap();
                assert_eq!(
                    outputs.len(),
                    tasks.completed_instances(discovery_output).unwrap().len()
                );
                assert_eq!(
                    roots.transformations.load(Ordering::Relaxed),
                    outputs.len() as u64
                );
                let mut total = 0;
                let before = roots.transformations.load(Ordering::Relaxed);
                for output in &outputs {
                    let observed = roots.materialize(output.clone()).unwrap();
                    assert_eq!(observed.recipes.len(), 10);
                    total += observed.initial.observations.len();
                    let mut wrong = output.clone();
                    wrong.action = store::Action(hirsel_syntax::ContentHash::of_bytes(
                        b"forged ownership completion",
                    ));
                    assert!(roots.materialize(wrong).is_err());
                }
                assert!(total >= 3, "actual resource create, move and drop");
                assert_eq!(
                    roots.transformations.load(Ordering::Relaxed),
                    before,
                    "readiness cannot lower again"
                );
                if ownership == 4 {
                    roots
                        .with_ready(root.clone(), |units, emit, plans| {
                            assert_eq!(plans.len(), units.len());
                            assert!(plans.values().all(|plan| plan.selection
                                == hirsel_lir::ownership_trace::Selection::Resources));
                            let llvm =
                                hirsel_llvm::emit_object_resources_ownership(units, emit, plans)
                                    .map_err(|e| e.to_string())?;
                            let clif = hirsel_cranelift::emit_object_resources_ownership(
                                units, emit, plans,
                            )
                            .map_err(|e| e.to_string())?;
                            assert!(!llvm.bytes.is_empty() && !clif.bytes.is_empty());
                            let mut missing = plans.clone();
                            missing.pop_first();
                            assert!(
                                hirsel_llvm::emit_object_resources_ownership(units, emit, &missing)
                                    .is_err()
                            );
                            assert!(
                                hirsel_cranelift::emit_object_resources_ownership(
                                    units, emit, &missing
                                )
                                .is_err()
                            );
                            Ok(())
                        })
                        .unwrap();
                }
                let mut wrong = root;
                wrong.action = store::Action(hirsel_syntax::ContentHash::of_bytes(
                    b"stale ownership root",
                ));
                assert!(roots.outputs(wrong).is_err());
                continue;
            }
            if assemble {
                for (key, _) in tasks.completed_instances((result[0]).clone()).unwrap() {
                    let request = ready_runtime.request(&[key.ready_query_id()]);
                    ready_runtime.wait(request).unwrap_or_else(|error| {
                        panic!(
                            "cross-owner Ready {}: {}",
                            key.instance.definition(),
                            String::from_utf8_lossy(&store.get_content(error).unwrap()),
                        )
                    });
                }
                use native::queries::NativeProducts;
                let mut products = native::queries::products::DemandProducts::new(snapshot.clone());
                let selected = tasks
                    .completed_instances((result[0]).clone())
                    .unwrap()
                    .iter()
                    .map(|(key, _)| products.ready(key).unwrap())
                    .collect::<Vec<_>>();
                products
                    .with_ready(&selected, |units, _| {
                        assert!(!units.is_empty());
                        Ok(())
                    })
                    .unwrap();
            }
            let selected = ready_runtime.request(&[main.ready_query_id()]);
            let ready = ready_runtime.wait(selected).unwrap_or_else(|error| {
                panic!(
                    "Ready phase: {error:?}, {:?}: {}",
                    ready_runtime.stats(),
                    String::from_utf8_lossy(&store.get_content(error).unwrap())
                )
            });
            assert_eq!(ready.len(), 1);
            let transformations = ready_tasks.ready_transformations.load(Ordering::Relaxed);
            if adapter && !fresh {
                assert!(transformations > 0);
            } else {
                assert_eq!(transformations, u64::from(!fresh || recompute));
            }
            assert_eq!(snapshot.ready_materializations(), 0);
            let completed = tasks.completed_instances((result[0]).clone()).unwrap();
            let specializations = completed
                .iter()
                .filter(|(key, _)| {
                    completed.iter().any(|(original, _)| {
                        key.instance != original.instance
                            && key.instance.is_same_or_derived_from(&original.instance)
                    })
                })
                .count();
            assert_eq!(specializations, expected_specializations);
            let original_count = completed.len() - specializations;
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
                .map(|(_, output)| output.product.product.clone())
                .collect::<Vec<_>>();
            if fresh {
                assert_eq!(expected.as_ref(), Some(&outputs));
                if recompute_instances {
                    assert_eq!(tasks.imports.load(Ordering::Relaxed), original_count as u64);
                    assert!(tasks.source_body_demands.load(Ordering::Relaxed) > 0);
                    assert_ne!(trace.totals(), Default::default());
                } else {
                    assert_eq!(tasks.imports.load(Ordering::Relaxed), 0);
                    assert_eq!(tasks.source_body_demands.load(Ordering::Relaxed), 0);
                    assert_eq!(trace.totals(), Default::default());
                }
            } else {
                assert_eq!(tasks.imports.load(Ordering::Relaxed), original_count as u64);
                expected = Some(outputs);
            }
        }
    });
}

const ORIGINAL_KEYED_MAP: &str = "fn main()->u64 {let m:Map[u64,u64]=map.empty();let get:fn(Map[u64,u64],u64)->Option[u64]=map.get;match get(m,1u64) {Some(x)=>x,None=>0u64}}";
fn original_inventory_mutation(mutation: u8) {
    ready_case_inner_mutation(
        ORIGINAL_KEYED_MAP,
        true,
        true,
        false,
        native::Validation::Ready,
        store::VerifyStore::Off,
        0,
        mutation,
        0,
    );
}
#[test]
fn original_keyed_map_missing_inventory_refuses_before_materialization() {
    original_inventory_mutation(1);
}
#[test]
fn original_keyed_map_forged_adapter_abi_refuses_before_materialization() {
    original_inventory_mutation(2);
}
#[test]
fn original_keyed_map_duplicate_inventory_refuses_before_materialization() {
    original_inventory_mutation(3);
}
#[test]
fn original_keyed_map_missing_source_body_refuses_before_materialization() {
    original_inventory_mutation(4);
}
