use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;
#[test]
fn admitted_source_growth_demands_unexecuted_required_roots() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(
            &path,
            "fn grow[T: copyable](x:T) { grow((copy x,copy x)) }\npub fn main(){}\n",
        )
        .unwrap();
        for jobs in [1, 2] {
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
            let mut grow = program.entry.clone();
            grow.0.name = "grow".into();
            let definitions = vec![grow, program.entry.clone()];
            let budget = ProcessBudget::new(1 << 30);
            let weight = MemoryWeight {
                retained: 1 << 20,
                scratch: 8 << 20,
                stack: 0,
            };
            let errors = crate::scheduled_front::errors();
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
            runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
            runtime
                .register(FRONT_GROWTH_KIND, growth.factory())
                .unwrap();
            let request = runtime.request(&[growth.query()]);
            let products = runtime.wait(request).unwrap();
            let output = growth.outcome(products[0]).unwrap();
            assert_eq!(output.diagnostics.len(), 1);
            assert_eq!(output.diagnostics[0].rule.as_str(), "GEN-5");
            assert!(
                program
                    .engine
                    .sources
                    .slice(output.diagnostics[0].primary)
                    .unwrap()
                    .contains("(copy x,copy x)")
            );
            let expected = program.engine.generic_graph_diagnostics().unwrap();
            assert_eq!(&*output.diagnostics, &expected);
            assert_eq!(
                runtime.stats().claims,
                3,
                "global growth plus both source roots, including unexecuted grow"
            );
            runtime.shutdown();
        }
    });
}
