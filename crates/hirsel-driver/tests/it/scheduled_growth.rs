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
                let output = growth.outcome((products[0]).clone()).unwrap();
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
                    let output = growth.outcome((products[0]).clone()).unwrap();
                    assert!(output.diagnostics.is_empty());
                    assert!(
                        output.facets.components.iter().any(|component| component
                            .members
                            .iter()
                            .any(|id| id.path().0.name == "relay")),
                        "actual generic definitions must reach the component root"
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

#[test]
fn changed_source_member_stops_at_equal_settled_component() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"sourcefacets","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let store = Arc::new(MemStore::default());
        let settle = |source: &str, jobs: usize| {
            std::fs::write(&path, source).unwrap();
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: store.clone(),
                    jobs: hirsel_intern::jobs::Jobs::new(jobs),
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let definitions = program.engine.front_source_roots();
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
            let selected = runtime.wait(request).unwrap();
            let output = growth.outcome((selected[0]).clone()).unwrap();
            assert_eq!(
                runtime.stats().claims,
                definitions.len() as u64 + 1,
                "every required source root still participates"
            );
            assert_eq!(
                &*output.diagnostics,
                &program.engine.generic_graph_diagnostics().unwrap()
            );
            runtime.shutdown();
            output
        };
        let source = "fn left[T:copyable](x:T){right(x)}\nfn right[T:copyable](x:T){left(x)}\nfn separate[T](x:T){separate(x)}\npub fn main(){}\n";
        let first = settle(source, 1);
        let changed = settle(&source.replace("{right(x)}", "{left(copy x);right(x)}"), 2);
        assert!(first.diagnostics.is_empty());
        assert!(changed.diagnostics.is_empty());
        let component = |output: &FrontGrowthOutput, name: &str| {
            output
                .facets
                .components
                .iter()
                .find(|component| component.members.iter().any(|id| id.path().0.name == name))
                .unwrap()
                .product
                .clone()
        };
        let before = component(&first, "left");
        let after = component(&changed, "left");
        assert_eq!(before.address, after.address);
        assert_ne!(
            before.action, after.action,
            "the changed member reaches the SCC"
        );
        assert_eq!(
            before
                .archive(ArchiveAdmission::Retained)
                .unwrap()
                .reference(),
            after
                .archive(ArchiveAdmission::Retained)
                .unwrap()
                .reference(),
            "equal settled facet stops propagation"
        );
        assert_eq!(
            component(&first, "separate"),
            component(&changed, "separate"),
            "the unrelated component observes no changed output"
        );
        let rejected = settle(&source.replace("{right(x)}", "{right((copy x,copy x))}"), 1);
        assert_eq!(rejected.diagnostics.len(), 1);
        assert_eq!(rejected.diagnostics[0].rule.as_str(), "GEN-5");
        assert_ne!(
            component(&rejected, "left")
                .archive(ArchiveAdmission::Retained)
                .unwrap()
                .reference(),
            before
                .archive(ArchiveAdmission::Retained)
                .unwrap()
                .reference(),
            "unexecuted growing roots still invalidate the verdict"
        );
        let split = settle(&source.replace("{left(x)}", "{let _=x}"), 2);
        assert!(split.diagnostics.is_empty());
        assert_ne!(
            component(&split, "left").address,
            before.address,
            "a split component cannot reuse the old membership"
        );
        let merged = settle(&source.replace("{right(x)}", "{right((copy x,copy x))}"), 2);
        assert_eq!(
            component(&merged, "left")
                .archive(ArchiveAdmission::Retained)
                .unwrap()
                .reference(),
            component(&rejected, "left")
                .archive(ArchiveAdmission::Retained)
                .unwrap()
                .reference()
        );
        assert_eq!(merged.diagnostics.len(), 1);
        let removed = settle("fn separate[T](x:T){separate(x)}\npub fn main(){}\n", 1);
        assert!(removed.diagnostics.is_empty());
        assert!(
            removed
                .facets
                .components
                .iter()
                .flat_map(|component| component.members.iter())
                .all(|id| !["left", "right"].contains(&id.path().0.name.as_str()))
        );
        for (source, expected, jobs) in [
            (
                "fn grow[T:copyable](x:T){grow((copy x,copy x))}\nfn other[T:copyable](x:T){other((copy x,copy x))}\npub fn main(){}\n",
                "grow",
                1,
            ),
            (
                "fn other[T:copyable](x:T){other((copy x,copy x))}\nfn grow[T:copyable](x:T){grow((copy x,copy x))}\npub fn main(){}\n",
                "other",
                2,
            ),
        ] {
            let output = settle(source, jobs);
            assert_eq!(
                output.diagnostics.len(),
                2,
                "both unexecuted cycles are rejected"
            );
            let owners: Vec<_> = output
                .diagnostics
                .iter()
                .map(|diagnostic| {
                    output
                        .facets
                        .components
                        .iter()
                        .find(|component| component.growing.contains(&diagnostic.primary))
                        .unwrap()
                        .members[0]
                        .path()
                        .0
                        .name
                        .clone()
                })
                .collect();
            let second = if expected == "grow" { "other" } else { "grow" };
            assert_eq!(
                owners,
                [expected, second],
                "source order, not query completion order, orders every diagnostic"
            );
        }
    });
}
