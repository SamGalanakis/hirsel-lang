use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;
struct Refuse(hirsel_engine::store::Reference);
impl LocalQueryFactory for Refuse {
    fn weight(&self, _: QueryId) -> Result<MemoryWeight, hirsel_engine::store::Reference> {
        Err(self.0)
    }
    fn create(
        &self,
        _: QueryId,
    ) -> Result<Box<dyn LocalQueryTask>, hirsel_engine::store::Reference> {
        panic!("refused query must never be created")
    }
}
impl QueryFactory for Refuse {
    fn create(&self, _: QueryId) -> Result<Box<dyn QueryTask>, hirsel_engine::store::Reference> {
        Err(self.0)
    }
}
#[test]
fn lower_forwards_exact_runtime_dependency_refusals_without_fabricating_a_body_failure() {
    refusal_case(FRONT_EVALUATE_KIND);
}
#[test]
fn lower_forwards_exact_front_runtime_refusals_before_typed_body_selection() {
    refusal_case(FRONT_BODY_KIND);
}
fn refusal_case(refused_kind: u32) {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("hirsel.json");
        std::fs::write(
            dir.path().join("main.hir"),
            "const A:u8=1u8;\npub fn main()->u8{A}\n",
        )
        .unwrap();
        std::fs::write(&manifest,r#"{"schema":1,"package":{"name":"refusal","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let errors = crate::scheduled_front::errors();
        for jobs in [1, 2] {
            for refused in [
                errors.resource,
                errors.cycle,
                errors.cancelled,
                errors.protocol,
            ] {
                let mut program = COMPILER
                    .with_config(CompileConfig {
                        store: Arc::new(MemStore::default()),
                        ..Default::default()
                    })
                    .with_owned_engines()
                    .load_program(&manifest)
                    .unwrap();
                let definitions = program.engine.front_source_roots();
                let budget = ProcessBudget::new(1 << 30);
                let weight = MemoryWeight {
                    retained: 2 << 20,
                    scratch: 8 << 20,
                    stack: 0,
                };
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
                        budget.reserve(16 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                let mut runtime = AdmittedRuntime::new(
                    hirsel_intern::jobs::Jobs::new(jobs),
                    budget.reserve(512 << 20).unwrap(),
                    errors,
                )
                .unwrap();
                if refused_kind == FRONT_BODY_KIND {
                    runtime
                        .register(FRONT_BODY_KIND, Arc::new(Refuse(refused)))
                        .unwrap();
                } else {
                    runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
                }
                runtime
                    .register_local(FRONT_LOWER_KIND, lower.factory())
                    .unwrap();
                if refused_kind == FRONT_EVALUATE_KIND {
                    runtime
                        .register_local(FRONT_EVALUATE_KIND, Arc::new(Refuse(refused)))
                        .unwrap();
                } else {
                    runtime
                        .register_local(FRONT_EVALUATE_KIND, lower.factory())
                        .unwrap();
                }
                let query = lower.query(&program.entry).unwrap();
                let request = runtime.request(&[query]);
                let actual = runtime.wait(request).unwrap_err();
                assert_eq!(
                    actual,
                    refused,
                    "preserve runtime refusal, jobs={jobs}; fabricated error={:?}",
                    lower.failure(actual)
                );
                assert!(
                    lower.failure(actual).is_none(),
                    "a scheduler refusal is not a typed body failure"
                );
                assert!(lower.settled_definition(&program.entry).is_err());
                assert_eq!(lower.work(&program.entry).lowerings, 0);
                runtime.shutdown();
            }
        }
    });
}
