use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;
#[test]
fn admitted_staged_declarations_match_scalar_without_executing_or_publishing() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(
            &source,
            "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\npub fn main(){}\n",
        )
        .unwrap();
        std::fs::write(&manifest,r#"{"schema":1,"package":{"name":"stage","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        for jobs in [1, 2] {
            for output_bytes in [1, 64 << 10] {
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
                let scaled = definitions
                    .iter()
                    .find(|p| p.0.name == "scaled")
                    .unwrap()
                    .clone();
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
                        budget.reserve(8 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                let input_bytes = FrontStageQueries::input_bytes(&definitions);
                let stage = FrontStageQueries::new(
                    &definitions,
                    front.clone(),
                    weight,
                    budget.reserve(input_bytes + output_bytes).unwrap(),
                    errors,
                )
                .unwrap();
                assert_eq!(stage.retained_bytes(), input_bytes);
                runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
                runtime.register(FRONT_STAGE_KIND, stage.factory()).unwrap();
                let query = stage.query(&scaled).unwrap();
                // A factory has no session/engine to install declarations into.
                drop(program);
                let request = runtime.request(&[query, query]);
                let result = runtime.wait(request);
                assert_eq!(front.live_body_count(), 0);
                let held = if output_bytes == 1 {
                    assert_eq!(result.unwrap_err(), errors.resource);
                    None
                } else {
                    let products =
                        result.unwrap_or_else(|r| panic!("stage {r:?}: {:?}", stage.failure(r)));
                    assert_eq!(products[0], products[1]);
                    let output = stage.outcome(products[0]).unwrap();
                    assert_eq!(output.owner(), &scaled);
                    assert!(!output.reads().is_empty());
                    let declarations = output.declarations().unwrap().collect::<Vec<_>>();
                    assert_eq!(declarations.len(), 1);
                    let mut cold = COMPILER
                        .with_config(CompileConfig {
                            store: Arc::new(MemStore::default()),
                            ..Default::default()
                        })
                        .with_owned_engines()
                        .load_program(&manifest)
                        .unwrap();
                    cold.engine.register_staged_definition(&scaled).unwrap();
                    for (path, signature) in declarations {
                        let expected = cold.engine.item_signature(path).unwrap();
                        assert_eq!(signature.fingerprint, expected.value.fingerprint);
                        assert_eq!(signature, &*expected.value);
                    }
                    assert!(stage.retained_bytes() > input_bytes);
                    assert!(stage.retained_bytes() <= input_bytes + output_bytes);
                    Some(output.clone())
                };
                runtime.shutdown();
                drop(runtime);
                drop(stage);
                drop(front);
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
fn staged_acceptance_checks_all_reads_and_admits_the_installed_copy() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        let source = directory.path().join("main.hir");
        std::fs::write(
            &source,
            "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\npub fn main(){}\n",
        )
        .unwrap();
        std::fs::write(&manifest,r#"{"schema":1,"package":{"name":"stage","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
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
        let scaled = definitions
            .iter()
            .find(|path| path.0.name == "scaled")
            .unwrap();
        let budget = ProcessBudget::new(1 << 30);
        let errors = crate::scheduled_front::errors();
        let weight = MemoryWeight {
            retained: 64 << 10,
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
        let stage = FrontStageQueries::new(
            &definitions,
            front.clone(),
            weight,
            budget.reserve(1 << 20).unwrap(),
            errors,
        )
        .unwrap();
        let mut runtime = AdmittedRuntime::new(
            hirsel_intern::jobs::Jobs::new(2),
            budget.reserve(512 << 20).unwrap(),
            errors,
        )
        .unwrap();
        runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
        runtime.register(FRONT_STAGE_KIND, stage.factory()).unwrap();
        let request = runtime.request(&[stage.query(scaled).unwrap()]);
        let products = runtime.wait(request).unwrap();
        let output = stage.outcome(products[0]).unwrap();
        let (path, expected) = output.declarations().unwrap().next().unwrap();
        let path = path.clone();
        let expected = expected.clone();
        assert!(
            program
                .engine
                .signature_snapshot()
                .signature(hirsel_syntax::DefId::intern(&path))
                .is_none()
        );
        assert!(
            program
                .engine
                .accept_front_staged(&output, budget.reserve(1).unwrap())
                .is_err()
        );
        assert!(
            program
                .engine
                .signature_snapshot()
                .signature(hirsel_syntax::DefId::intern(&path))
                .is_none()
        );
        program
            .engine
            .accept_front_staged(&output, budget.reserve(64 << 10).unwrap())
            .unwrap();
        assert_eq!(
            *program.engine.item_signature(&path).unwrap().value,
            expected
        );
        runtime.shutdown();
        drop(runtime);
        drop(output);
        drop(stage);
        drop(front);
        assert!(
            budget.reserved() > 0,
            "installed source must retain its own grant"
        );
        drop(program);
        assert_eq!(budget.reserved(), 0);
    });
}
