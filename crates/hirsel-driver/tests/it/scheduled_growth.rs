use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;
#[test]
fn admitted_source_growth_demands_unexecuted_required_roots() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        for (source, expected_rule) in [
            (
                "fn grow[T: copyable](x:T) { grow((copy x,copy x)) }\npub fn main(){}\n",
                Some("GEN-5"),
            ),
            (
                include_str!("../../../../conformance/STORE/STORE-6-generic-key-bad.hir"),
                Some("STORE-6"),
            ),
            (
                include_str!("../../../../conformance/STORE/STORE-6-generic-key-ok.hir"),
                None,
            ),
        ] {
            std::fs::write(&path, source).unwrap();
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
                let mut definitions = program.engine.front_source_roots();
                loop {
                    program.engine.register_specializations(&definitions);
                    let selected = program.engine.front_source_roots();
                    if selected == definitions {
                        break;
                    }
                    definitions = selected;
                }
                if expected_rule == Some("GEN-5") {
                    assert!(
                        definitions.iter().any(|path| path.0.name == "grow"),
                        "unexecuted declaration remains required"
                    );
                }
                let budget = ProcessBudget::new(1 << 30);
                let weight = MemoryWeight {
                    retained: 64 << 10,
                    scratch: 8 << 20,
                    stack: 0,
                };
                let errors = crate::scheduled_front::errors();
                let mut runtime = AdmittedRuntime::with_query_capacity(
                    hirsel_intern::jobs::Jobs::new(jobs),
                    budget.reserve(512 << 20).unwrap(),
                    errors,
                    definitions.len() + 1,
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
                    .unwrap_or_else(|error| panic!("{} roots: {error:?}", definitions.len()));
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
                assert_eq!(
                    output.diagnostics.len(),
                    usize::from(expected_rule.is_some())
                );
                if let Some(rule) = expected_rule {
                    assert_eq!(output.diagnostics[0].rule.as_str(), rule);
                    if rule == "GEN-5" {
                        assert!(
                            program
                                .engine
                                .sources
                                .slice(output.diagnostics[0].primary)
                                .unwrap()
                                .contains("(copy x,copy x)")
                        );
                    }
                }
                let expected = program.engine.generic_graph_diagnostics().unwrap();
                assert_eq!(&*output.diagnostics, &expected);
                assert_eq!(
                    runtime.stats().claims,
                    definitions.len() as u64 + 1,
                    "global growth plus every required source root, including unexecuted grow"
                );
                runtime.shutdown();
            }
        }
    });
}

#[test]
fn growth_output_admission_does_not_spend_its_grant_on_the_runtime_frame() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&source,"fn leaf[T:copyable](x:T)->T{x}\nfn relay[T:copyable](x:T)->T{leaf(x)}\npub fn main(){}\n").unwrap();
        std::fs::write(&manifest,r#"{"schema":1,"package":{"name":"growth","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        for jobs in [1, 2] {
            for output_bytes in [1, 16 << 10] {
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
                let errors = crate::scheduled_front::errors();
                let weight = MemoryWeight {
                    retained: 16 << 10,
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
                let inputs =
                    FrontGrowthQueries::input_bytes(&definitions, MemoryWeight::default()).unwrap();
                let growth = program
                    .engine
                    .front_growth_queries(
                        &program.entry,
                        front.clone(),
                        weight,
                        budget.reserve(inputs + output_bytes).unwrap(),
                        errors,
                    )
                    .unwrap();
                runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
                runtime
                    .register(FRONT_GROWTH_KIND, growth.factory())
                    .unwrap();
                let request = runtime.request(&[growth.query()]);
                let result = runtime.wait(request);
                let held = if output_bytes == 1 {
                    assert_eq!(result.unwrap_err(), errors.resource);
                    None
                } else {
                    let products = result.unwrap();
                    let output = growth.outcome(products[0]).unwrap();
                    assert!(output.diagnostics.is_empty());
                    assert!(
                        !output.growth.edges.is_empty(),
                        "actual generic calls must reach Growth"
                    );
                    assert_eq!(runtime.stats().claims, definitions.len() as u64 + 1);
                    Some(output.clone())
                };
                assert_eq!(front.live_body_count(), 0);
                runtime.shutdown();
                drop(runtime);
                drop(program);
                drop(front);
                drop(growth);
                if held.is_some() {
                    assert!(budget.reserved() > 0);
                }
                drop(held);
                assert_eq!(budget.reserved(), 0);
            }
        }
    });
}
