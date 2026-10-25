use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

fn scope_case(source: &str, failed_front: bool, failed_lower: bool) {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("main.hir"), source).unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"scope","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        for jobs in [1, 2] {
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: Arc::new(MemStore::default()),
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
                retained: 2 << 20,
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
                    budget.reserve(8 << 20).unwrap(),
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
                    budget.reserve(8 << 20).unwrap(),
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
                    weight,
                    budget.reserve(8 << 20).unwrap(),
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
            let query = check.query();
            let unknown = ProductRef {
                address: query.address,
                action: hirsel_artifact_store::Action(query.address.0),
                record: errors.protocol,
            };
            assert!(
                check
                    .completed_scope(unknown, budget.reserve(1 << 20).unwrap())
                    .is_err()
            );
            let request = runtime.request(&[query]);
            let product = runtime.wait(request).unwrap()[0];
            let output = check.outcome(product).unwrap();
            assert_eq!(output.checked.completion, CheckCompletion::Complete);
            assert_eq!(
                output
                    .checked
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == hirsel_diag::Severity::Error),
                failed_front || failed_lower
            );
            assert!(
                check
                    .completed_scope(unknown, budget.reserve(1 << 20).unwrap())
                    .is_err()
            );
            assert!(
                check
                    .completed_scope(product, budget.reserve(0).unwrap())
                    .is_err()
            );
            let scope = check
                .completed_scope(product, budget.reserve(1 << 20).unwrap())
                .unwrap();
            assert!(!scope.reads().is_empty());
            assert!(scope.reads().values().all(|r| !r.is_empty()));
            if failed_front {
                assert!(
                    scope.reads().contains_key(&program.entry),
                    "a failed P root retains its negative reads"
                );
                assert!(scope.lowered().is_empty());
            } else if failed_lower {
                let mut bad = program.entry.clone();
                bad.0.name = "BAD".into();
                let mut later = program.entry.clone();
                later.0.name = "LATER".into();
                assert!(scope.failed_lowered().contains(&bad));
                assert!(scope.reads().contains_key(&bad));
                assert!(!scope.lowered().contains(&later));
                assert_eq!(lower.work(&later).initializers, 0);
                let before = lower.work(&bad);
                assert!(
                    scope.definition(&bad).is_err(),
                    "failed root cannot demand a body"
                );
                assert_eq!(lower.work(&bad), before);
            } else {
                let mut constant = program.entry.clone();
                constant.0.name = "A".into();
                assert!(scope.lowered().contains(&constant));
                assert!(scope.failed_lowered().is_empty());
                let before = lower.work(&constant);
                assert!(scope.definition(&constant).is_ok());
                assert_eq!(
                    lower.work(&constant),
                    before,
                    "inspection decodes only settled products"
                );
            }
            assert!(scope.definition(&DefPath::core("unselected")).is_err());
            assert_eq!(front.live_body_count(), 0);
            let expected = program.check(LoweringPolicy::Comptime).unwrap();
            assert_eq!(expected.completion, output.checked.completion);
            let expected = program.engine.checked_products(&program.entry).unwrap();
            let canonical = |reads: &hirsel_engine::Reads| {
                reads
                    .iter()
                    .map(|(key, value)| {
                        let mut bytes = Vec::new();
                        key.encode(&mut bytes);
                        (bytes, *value)
                    })
                    .collect::<std::collections::BTreeMap<_, _>>()
            };
            let actual = scope
                .reads()
                .iter()
                .map(|(path, reads)| (path.clone(), canonical(reads)))
                .collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(
                actual, expected.reads,
                "the completed S5 scope preserves scalar complete reads, including failure obligations"
            );
            runtime.shutdown();
            drop(program);
            assert!(
                !scope.reads().is_empty(),
                "scope owns immutable producers after shutdown"
            );
        }
    });
}
#[test]
fn completed_check_scope_inspects_only_successful_required_lowerings() {
    scope_case(
        "const A:u8=comptime 1u8; pub fn main()->u8{A}",
        false,
        false,
    );
}
#[test]
fn completed_check_scope_preserves_failed_front_negative_reads() {
    scope_case("pub fn main()->u8{missing}", true, false);
}
#[test]
fn completed_check_scope_guards_failed_lower_and_withheld_suffix() {
    scope_case(
        "const BAD:u8=comptime(1u8/0u8); const LATER:u8=comptime 2u8; pub fn main()->u8{0u8}",
        false,
        true,
    );
}
