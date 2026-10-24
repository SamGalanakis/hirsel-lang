use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;
#[test]
fn ordered_check_gates_lowering_and_preserves_file_failure_prefixes() {
    with_compiler_stack(|| {
        for (source, reject_growth, fail_lower) in [
            (
                "const A:u8=comptime 1u8;\nconst B:u8=comptime A;\npub fn main()->u8{B}\n",
                false,
                false,
            ),
            (
                "const A:u8=comptime 1u8;\nfn grow[T: copyable](x:T){grow((copy x,copy x))}\npub fn main(){}\n",
                true,
                false,
            ),
            (
                "const BAD:u8=comptime(1u8/0u8);\nconst LATER:u8=comptime 2u8;\npub fn main()->u8{0u8}\n",
                false,
                true,
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("main.hir");
            std::fs::write(&path, source).unwrap();
            for jobs in [1, 2] {
                for policy in [LoweringPolicy::Comptime, LoweringPolicy::Reachable] {
                    let store = Arc::new(MemStore::default());
                    let mut program = COMPILER
                        .with_config(CompileConfig {
                            store: store.clone(),
                            jobs: hirsel_intern::jobs::Jobs::new(jobs),
                            ..Default::default()
                        })
                        .with_owned_engines()
                        .load_program(&path)
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
                    let front = program
                        .engine
                        .front_body_queries(
                            &definitions,
                            weight,
                            budget
                                .reserve((weight.retained + 8192) * definitions.len() as u64)
                                .unwrap(),
                            errors,
                        )
                        .unwrap();
                    let lower = program
                        .engine
                        .front_lower_queries(
                            &definitions,
                            front.clone(),
                            weight,
                            32,
                            budget.reserve(32 << 20).unwrap(),
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
                    let plan = FrontCheckPlan {
                        files: program
                            .files
                            .iter()
                            .map(|(file, context)| (*file, context.clone(), policy))
                            .collect(),
                        front: front.clone(),
                        lower: lower.clone(),
                        growth: growth.clone(),
                        unsafe_ffi: program.unsafe_ffi.clone(),
                    };
                    let check = program
                        .engine
                        .front_check_queries(
                            plan,
                            weight,
                            budget.reserve(16 << 20).unwrap(),
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
                    let request = runtime.request(&[check.query()]);
                    let actual = runtime
                        .wait(request)
                        .map(|products| check.outcome(products[0]).unwrap())
                        .map_err(|reference| {
                            check
                                .failure(reference)
                                .map(|(error, _)| error)
                                .or_else(|| lower.failure(reference))
                                .unwrap_or_else(|| panic!("unowned failure {reference:?}"))
                        });
                    if reject_growth {
                        let mut constant = program.entry.clone();
                        constant.0.name = "A".into();
                        assert_eq!(
                            lower.work(&constant).initializers,
                            0,
                            "GEN5 rejection withholds S1"
                        );
                    }
                    if fail_lower {
                        let mut later = program.entry.clone();
                        later.0.name = "LATER".into();
                        assert_eq!(
                            lower.work(&later).initializers,
                            0,
                            "failed earlier Lower withholds suffix"
                        );
                    }
                    let expected = program.check(policy);
                    match (actual, expected) {
                        (Ok(actual), Ok(expected)) => {
                            assert_eq!(actual.checked.completion, expected.completion);
                            assert_eq!(
                                actual.checked.diagnostics, expected.diagnostics,
                                "jobs={jobs},policy={policy:?}"
                            );
                        }
                        (Err(actual), Err(expected)) => assert_eq!(actual, expected),
                        (actual, expected) => panic!(
                            "different acceptance: actual={},expected={:?}",
                            actual.is_ok(),
                            expected.err()
                        ),
                    }
                    runtime.shutdown();
                }
            }
        }
    });
}
