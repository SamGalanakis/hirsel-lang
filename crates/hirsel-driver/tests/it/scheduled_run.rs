use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

#[test]
fn admitted_run_root_matches_scalar_semantic_and_launch_facts() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(
            &path,
            "const A:u8=comptime 3u8;\nfn helper()->u8{A}\nfn unreachable()->u8{9u8}\npub fn main()->u8{helper()}\n",
        )
        .unwrap();
        for jobs in [1, 2] {
            let store = Arc::new(MemStore::default());
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store,
                    jobs: hirsel_intern::jobs::Jobs::new(jobs),
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&path)
                .unwrap();
            let definitions = program.engine.front_source_roots();
            let budget = ProcessBudget::new(1 << 30);
            let errors = crate::scheduled_front::errors();
            let front_weight = MemoryWeight {
                retained: 64 << 10,
                scratch: 8 << 20,
                stack: 0,
            };
            let lower_weight = MemoryWeight {
                retained: 64 << 10,
                ..front_weight
            };
            let growth_weight = MemoryWeight {
                retained: 64 << 10,
                ..front_weight
            };
            let run_weight = MemoryWeight {
                retained: 16 << 20,
                ..front_weight
            };
            let mut runtime = AdmittedRuntime::with_query_capacity(
                hirsel_intern::jobs::Jobs::new(jobs),
                budget.reserve(512 << 20).unwrap(),
                errors,
                definitions.len() * 4 + 2,
            )
            .unwrap();
            let front = program
                .engine
                .front_body_queries(
                    &definitions,
                    front_weight,
                    budget
                        .reserve((front_weight.retained + 8192) * definitions.len() as u64)
                        .unwrap(),
                    errors,
                )
                .unwrap();
            let lower = program
                .engine
                .front_lower_queries(
                    &definitions,
                    front.clone(),
                    lower_weight,
                    4096,
                    budget.reserve(32 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let growth = program
                .engine
                .front_growth_queries(
                    &program.entry,
                    front.clone(),
                    growth_weight,
                    budget.reserve(16 << 20).unwrap(),
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
                    },
                    front_weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let run = program
                .engine
                .front_run_queries(
                    check.clone(),
                    front.clone(),
                    lower.clone(),
                    run_weight,
                    budget.reserve(64 << 20).unwrap(),
                    errors,
                )
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
            runtime.register(FRONT_CHECK_KIND, check.factory()).unwrap();
            runtime.register(FRONT_RUN_KIND, run.factory()).unwrap();
            let request = runtime.request(&[run.query(&program.entry).unwrap()]);
            let products = runtime.wait(request).unwrap_or_else(|reference| {
                panic!(
                    "Run failure {reference:?}: run={:?},check={:?},lower={:?},front={:?}",
                    run.failure(reference),
                    check.failure(reference),
                    lower.failure(reference),
                    front.outcome(reference).map(|o| o.outcome)
                )
            });
            let output = run.outcome(products[0]).unwrap();
            assert_eq!(
                output.revision,
                program
                    .engine
                    .semantic_program_revision(&program.entry)
                    .unwrap()
            );
            assert_eq!(
                output
                    .semantic
                    .semantic_program_revision(&program.entry)
                    .unwrap(),
                output.revision
            );
            assert_eq!(
                output.foreign,
                program.engine.foreign_use(&program.entry).unwrap()
            );
            assert_eq!(
                output.code,
                program
                    .engine
                    .unsafe_reach(&program.entry)
                    .unwrap()
                    .map_or(hirsel_run_schema::CodeReach::Safe, |r| {
                        hirsel_run_schema::CodeReach::Escapes(r.spelled())
                    })
            );
            assert_eq!(
                output.callees,
                hirsel_engine::callee_facts(&mut program.engine, &program.entry).unwrap()
            );
            assert_eq!(
                output.entries,
                hirsel_engine::served_entries(&mut program.engine, &program.entry).unwrap()
            );
            assert!(Arc::ptr_eq(
                output.semantic.signatures(),
                &lower.signatures()
            ));
            let sources = output
                .semantic
                .source_bindings()
                .map(|(p, _, _)| p.clone())
                .collect::<Vec<_>>();
            assert!(sources.contains(&program.entry));
            assert!(!sources.iter().any(|p| p.0.name == "unreachable"));
            let before = lower.work_snapshot();
            runtime.shutdown();
            drop(run);
            drop(front);
            drop(growth);
            drop(check);
            assert_eq!(
                lower.work_snapshot(),
                before,
                "reading launch metadata runs no phase"
            );
            assert!(
                budget.reserved() > 0,
                "external launch output retains admission"
            );
            drop(lower);
            drop(output);
            assert_eq!(
                budget.reserved(),
                0,
                "no completed-output/owner reference cycle"
            );
        }
    });
}
