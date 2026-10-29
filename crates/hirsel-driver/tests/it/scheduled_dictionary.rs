use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

#[test]
fn admitted_dictionary_matches_scalar_named_callables_and_resumes_data_dependencies_once() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&source, "type Op { run:fn(u64)->u64 uses {}, tag:u64 }\nfn identity(x:u64)->u64 uses {} {x}\nfn seed()->u64 uses {} {7u64}\nconst OP:Op=Op(run:identity,tag:seed())\npub fn main()->u64 {7u64}\n").unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"dictionary","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        for jobs in [1, 2] {
            let store = Arc::new(MemStore::default());
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: store.clone(),
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let paths = program.engine.front_source_roots();
            let dictionary = paths.iter().find(|p| p.0.name == "OP").unwrap().clone();
            let seed = paths.iter().find(|p| p.0.name == "seed").unwrap().clone();
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
                .front_body_queries(&paths, weight, budget.reserve(8 << 20).unwrap(), errors)
                .unwrap();
            let lower = program
                .engine
                .front_lower_queries(
                    &paths,
                    front.clone(),
                    weight,
                    3 * paths.len(),
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
            let request = runtime.request(&[
                lower.evaluate_query(&dictionary).unwrap(),
                lower.query(&dictionary).unwrap(),
            ]);
            let products = runtime
                .wait(request)
                .unwrap_or_else(|r| panic!("dictionary {r:?}: {:?}", lower.failure(r)));
            let actual = lower.settled_constant(&dictionary, &[]).unwrap();
            let codec = NativeRecords::new(program.engine.types().clone(), store.clone());
            let actual_bytes = codec.encode(&program.engine, &*actual).unwrap();
            let ordinary = lower.outcome((products[1]).clone()).unwrap();
            assert!(
                ordinary
                    .materialized
                    .iter()
                    .any(|(_, value)| codec.encode(&program.engine, value).as_deref()
                        == Some(actual_bytes.as_slice()))
            );
            let hirsel_tir::ValueKind::Construct { fields, .. } = &actual.root.kind else {
                panic!("not a dictionary");
            };
            assert!(fields.iter().any(|field| matches!(&field.kind, hirsel_tir::ValueKind::FunctionReference { definition, .. } if definition.0.name=="identity")));
            let work = lower.work_snapshot();
            assert_eq!(
                work.iter().find(|(p, _)| *p == seed).unwrap().1.lowerings,
                1,
                "the suspended data initializer demands one actual callee Lower"
            );
            assert_eq!(
                work.iter()
                    .find(|(p, _)| *p == dictionary)
                    .unwrap()
                    .1
                    .initializers,
                1,
                "suspension must not replay the field initializer"
            );

            let mut cold = COMPILER
                .with_config(CompileConfig {
                    store: Arc::new(MemStore::default()),
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let expected = cold.engine.materialize_value(&dictionary).unwrap();
            let expected_codec =
                NativeRecords::new(cold.engine.types().clone(), Arc::new(MemStore::default()));
            assert_eq!(
                actual_bytes,
                expected_codec.encode(&cold.engine, &expected).unwrap()
            );
            drop(actual);
            runtime.shutdown();
            assert!(runtime.stats().peak_bytes <= 512 << 20);
            drop(ordinary);
            drop(lower);
            drop(front);
            assert_eq!(budget.reserved(), 0);
        }
    });
}
