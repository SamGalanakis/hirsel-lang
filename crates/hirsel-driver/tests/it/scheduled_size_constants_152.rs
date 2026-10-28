use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

#[test]
fn admitted_size_instances_preserve_duplicate_roots_and_closed_values_after_restore() {
    check_size_instances(false);
}

#[test]
fn restored_parent_replays_nested_selections_without_selecting_dead_branch_instances() {
    check_size_instances(true);
}

fn check_size_instances(nested: bool) {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        let source = "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime (n*n)}\nconst A:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}\nconst B:u64=comptime {let xs:Inline[u8,4]=[1,2,3,4];scaled(xs)}\n";
        let source = if nested {
            format!(
                "{source}const C:u64=comptime {{if false {{let xs:Inline[u8,3]=[1,2,3];scaled(xs)}} else {{B+A+B}}}}\n"
            )
        } else {
            source.to_owned()
        };
        std::fs::write(directory.path().join("main.hir"), source).unwrap();
        std::fs::write(&manifest,r#"{"schema":1,"package":{"name":"stage","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[]}"#).unwrap();
        for jobs in [1, 2, 8] {
            let store = Arc::new(MemStore::default());
            let mut selected_hidden = None;
            for warm in [false, true] {
                let mut program = COMPILER
                    .with_config(CompileConfig {
                        store: store.clone(),
                        hold_bytes: 0,
                        ..Default::default()
                    })
                    .with_owned_engines()
                    .load_check_package(&manifest)
                    .unwrap();
                let definitions = program.engine.front_source_roots();
                let scaled = definitions
                    .iter()
                    .find(|path| path.0.name == "scaled")
                    .unwrap();
                let a = definitions.iter().find(|path| path.0.name == "A").unwrap();
                let b = definitions.iter().find(|path| path.0.name == "B").unwrap();
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
                        budget.reserve(16 << 20).unwrap(),
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
                runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
                runtime.register(FRONT_STAGE_KIND, stage.factory()).unwrap();
                let request = runtime.request(&[stage.query(scaled).unwrap()]);
                let products = runtime.wait(request).unwrap();
                let output = stage.outcome(products[0]).unwrap();
                program
                    .engine
                    .accept_front_staged(&output, budget.reserve(64 << 10).unwrap())
                    .unwrap();
                let declarations: Vec<_> = output
                    .declarations()
                    .unwrap()
                    .map(|(path, _)| path.clone())
                    .collect();
                runtime.shutdown();
                drop(runtime);
                drop(output);
                drop(stage);
                drop(front);
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
                        budget.reserve(32 << 20).unwrap(),
                        errors,
                    )
                    .unwrap();
                let mut runtime = AdmittedRuntime::new(
                    hirsel_intern::jobs::Jobs::new(jobs),
                    budget.reserve(512 << 20).unwrap(),
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
                let paths = if nested {
                    let c = definitions.iter().find(|path| path.0.name == "C").unwrap();
                    [c, c, c]
                } else {
                    [b, a, b]
                };
                let roots: Vec<_> = paths
                    .iter()
                    .map(|path| lower.evaluate_query(path).unwrap())
                    .collect();
                drop(program);
                let request = runtime.request(&roots);
                let products = runtime.wait(request).unwrap_or_else(|reference| {
                    panic!(
                        "jobs={jobs} warm={warm}: {reference:?}: {:?}",
                        lower.failure(reference)
                    )
                });
                assert_eq!(
                    products[0], products[2],
                    "duplicate roots select one immutable product"
                );
                let expected = if nested { [36, 36, 36] } else { [16, 4, 16] };
                for (path, expected) in paths.iter().zip(expected) {
                    let value = lower.settled_constant(path, &[]).unwrap();
                    assert_eq!(
                        value.root.kind,
                        hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(expected))
                    );
                }
                let hidden = if warm {
                    selected_hidden
                        .clone()
                        .expect("cold TIR selected its staged declaration")
                } else {
                    let generic = lower.settled_definition(scaled).unwrap();
                    let hidden = generic
                        .bodies
                        .iter()
                        .flat_map(|body| &body.rvalues)
                        .find_map(|value| match value {
                            hirsel_tir::Rvalue::ReadConstant {
                                definition,
                                arguments,
                            } if !arguments.is_empty() => Some(definition.clone()),
                            _ => None,
                        })
                        .expect("the original generic body retains its staged size read");
                    selected_hidden = Some(hidden.clone());
                    hidden
                };
                if warm {
                    for path in paths {
                        assert_eq!(
                            lower.work(path).initializers,
                            0,
                            "the restored root does not execute"
                        );
                    }
                    assert_eq!(
                        lower.work(a).initializers,
                        0,
                        "A restores its completed value"
                    );
                    assert_eq!(
                        lower.work(b).initializers,
                        0,
                        "B restores its completed value"
                    );
                }
                assert!(declarations.contains(&hidden));
                assert_eq!(
                    lower.settled_constant(&hidden, &[2]).unwrap().root.kind,
                    hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4)),
                    "jobs={jobs} warm={warm} selected={hidden}"
                );
                assert_eq!(
                    lower.settled_constant(&hidden, &[4]).unwrap().root.kind,
                    hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(16))
                );
                assert!(
                    lower.settled_constant(&hidden, &[3]).is_err(),
                    "an unselected instance stays absent"
                );
                runtime.shutdown();
                drop(runtime);
                drop(lower);
                drop(front);
                assert_eq!(budget.reserved(), 0);
            }
        }
    });
}
