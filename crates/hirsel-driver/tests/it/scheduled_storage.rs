use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(Default)]
struct RecipeClaims(AtomicU64);
impl RuntimeObserver for RecipeClaims {
    fn record(&self, event: RuntimeEvent) {
        if matches!(event, RuntimeEvent::Claim(query) if query.kind == FRONT_STORAGE_KIND) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[test]
fn admitted_source_storage_root_matches_scalar_and_rejects_root_escape() {
    with_compiler_stack(|| {
        for (source, reject) in [
            (
                "fn identity(x:u64)->u64{x}\npub fn main(mut r:Region[u64]){let n=region.with(mut r,identity)}\n",
                false,
            ),
            (
                "fn identity(root:List[u64])->unique List[u64]{unique root}\npub fn main(mut r:Region[List[u64]]){let escaped=region.with(mut r,identity)}\n",
                true,
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("main.hir");
            let manifest = directory.path().join("hirsel.json");
            std::fs::write(&path, source).unwrap();
            std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"storage","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
            for jobs in [1, 2] {
                let store = Arc::new(MemStore::default());
                let mut program = COMPILER
                    .with_config(CompileConfig {
                        store: store.clone(),
                        jobs: hirsel_intern::jobs::Jobs::new(jobs),
                        hold_bytes: 0,
                        ..Default::default()
                    })
                    .with_owned_engines()
                    .load_program(&manifest)
                    .unwrap();
                let definitions = program.engine.front_source_roots();
                let budget = ProcessBudget::new(1 << 30);
                let errors = crate::scheduled_front::errors();
                let weight = MemoryWeight {
                    retained: 64 << 10,
                    scratch: 8 << 20,
                    stack: 0,
                };
                let mut runtime = AdmittedRuntime::new(
                    hirsel_intern::jobs::Jobs::new(jobs),
                    budget.reserve(512 << 20).unwrap(),
                    errors,
                )
                .unwrap();
                let recipe_claims = Arc::new(RecipeClaims::default());
                runtime.observe(recipe_claims.clone()).unwrap();
                let front = program
                    .engine
                    .front_body_queries(
                        &definitions,
                        weight,
                        budget.reserve(16 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                let lower = program
                    .engine
                    .front_lower_queries(
                        &definitions,
                        front.clone(),
                        weight,
                        4096,
                        budget.reserve(16 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                let growth = program
                    .engine
                    .front_growth_queries(
                        &program.entry,
                        front.clone(),
                        weight,
                        budget.reserve(8 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                let recipes = program
                    .engine
                    .front_storage_queries(
                        front.clone(),
                        weight,
                        budget.reserve(16 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                assert!(
                    program
                        .engine
                        .front_storage_graph_queries(
                            &program.entry,
                            front.clone(),
                            growth.clone(),
                            recipes.clone(),
                            lower.clone(),
                            MemoryWeight {
                                scratch: weight.scratch - 1,
                                ..weight
                            },
                            budget.reserve(8 << 20).unwrap(),
                            errors,
                        )
                        .is_err(),
                    "a composed recipe cannot exceed its parent's scratch grant"
                );
                let graph = program
                    .engine
                    .front_storage_graph_queries(
                        &program.entry,
                        front.clone(),
                        growth.clone(),
                        recipes.clone(),
                        lower.clone(),
                        weight,
                        budget.reserve(8 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                let check = program
                    .engine
                    .front_check_queries(
                        FrontCheckPlan {
                            files: program
                                .files
                                .iter()
                                .map(|(file, context)| {
                                    (*file, context.clone(), LoweringPolicy::Comptime)
                                })
                                .collect(),
                            front: front.clone(),
                            lower: lower.clone(),
                            growth: growth.clone(),
                            unsafe_ffi: program.unsafe_ffi.clone(),
                            lint_package: None,
                        },
                        weight,
                        budget.reserve(16 << 20).unwrap(),
                        errors,
                    )
                    .unwrap()
                    .with_storage(graph.clone())
                    .unwrap();
                runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
                runtime
                    .register_local(FRONT_LOWER_KIND, lower.factory())
                    .unwrap();
                runtime
                    .register_local(FRONT_EVALUATE_KIND, lower.factory())
                    .unwrap();
                runtime
                    .register(FRONT_GROWTH_KIND, growth.factory())
                    .unwrap();
                runtime
                    .register(FRONT_STORAGE_KIND, recipes.factory())
                    .unwrap();
                runtime
                    .register(FRONT_STORAGE_GRAPH_KIND, graph.factory())
                    .unwrap();
                runtime.register(FRONT_CHECK_KIND, check.factory()).unwrap();
                let request = runtime.request(&[check.query()]);
                let product = runtime.wait(request).unwrap_or_else(|r| {
                    panic!(
                        "storage Check failure {r:?}: check={:?}, graph={:?}",
                        check.failure(r),
                        graph.failure(r)
                    )
                })[0]
                    .clone();
                let output = check.outcome(product).unwrap();
                assert_eq!(output.checked.completion, CheckCompletion::Complete);
                let expected = program.check(LoweringPolicy::Comptime).unwrap();
                assert_eq!(
                    output.checked.diagnostics, expected.diagnostics,
                    "jobs={jobs}, reject={reject}"
                );
                assert_eq!(
                    output
                        .checked
                        .diagnostics
                        .iter()
                        .any(|d| d.rule.as_str() == "MODE-1"),
                    reject
                );
                assert_eq!(
                    graph.executions(),
                    1,
                    "source diagnostics={:?}; roots={:?}; front={:?}",
                    output.checked.diagnostics,
                    definitions,
                    output
                        .front
                        .iter()
                        .map(|(path, _)| path)
                        .collect::<Vec<_>>()
                );
                assert!(recipes.executions() > 0, "actual source recipes demanded");
                assert_eq!(
                    recipe_claims.0.load(Ordering::Relaxed),
                    0,
                    "the storage root owns recipe continuations, not scheduler nodes"
                );
                let again = runtime.request(&[graph.query()]);
                let graph_product = runtime.wait(again).unwrap()[0].clone();
                let held = graph.outcome(graph_product).unwrap();
                assert_eq!(graph.executions(), 1, "settled root coalesces");
                if !reject {
                    assert!(!held.profiles.is_empty());
                }
                assert_eq!(
                    front.live_body_count(),
                    0,
                    "no source body retained by storage graph"
                );
                runtime.shutdown();
                drop(output);
                drop(check);
                drop(graph);
                drop(recipes);
                drop(growth);
                drop(lower);
                drop(front);
                assert!(budget.reserved() > 0, "held storage output owns admission");
                drop(held);
                assert_eq!(budget.reserved(), 0);
            }
        }
    });
}

#[test]
fn native_source_requests_complete_storage_and_preserve_root_escape_rejection() {
    with_compiler_stack(|| {
        for (source, reject) in [
            (
                "fn identity(x:u64)->u64{x}\npub fn main(mut r:Region[u64]){let n=region.with(mut r,identity)}\n",
                false,
            ),
            (
                "fn identity(root:List[u64])->unique List[u64]{unique root}\npub fn main(mut r:Region[List[u64]]){let escaped=region.with(mut r,identity)}\n",
                true,
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let manifest = directory.path().join("hirsel.json");
            std::fs::write(directory.path().join("main.hir"), source).unwrap();
            std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"storage","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
            for jobs in [1, 2] {
                let config = CompileConfig {
                    store: Arc::new(MemStore::default()),
                    jobs: hirsel_intern::jobs::Jobs::new(jobs),
                    hold_bytes: 0,
                    ..Default::default()
                };
                let compiler = COMPILER.with_config(config.clone());
                let program = compiler.load_program(&manifest).unwrap();
                let root = program.entry.clone();
                drop(program);
                let mut requests =
                    hirsel_driver::requests::CompilerRequests::for_host(compiler, config);
                let result = requests.execute(
                    &manifest,
                    hirsel_driver::requests::CompileOperation::NativeSources {
                        roots: vec![root.clone()],
                    },
                );
                if reject {
                    let Err(ToolError::Query {
                        error: QueryError::Diagnostics(diagnostics),
                        ..
                    }) = result
                    else {
                        panic!("expected source storage rejection at jobs={jobs}");
                    };
                    assert!(
                        diagnostics
                            .iter()
                            .any(|diagnostic| diagnostic.rule.as_str() == "MODE-1")
                    );
                } else {
                    let output = result.unwrap();
                    let hirsel_driver::requests::CompileResult::NativeSources(facts) =
                        &output.result
                    else {
                        panic!("expected completed original source products");
                    };
                    assert!(
                        facts
                            .originals
                            .iter()
                            .any(|product| product.definition == root)
                    );
                }
            }
        }
    });
}
