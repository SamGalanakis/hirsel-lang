use hirsel_driver::*;
use hirsel_engine::{
    runtime::*,
    store::{Domain, MemStore, ResultStore},
    *,
};
use std::sync::Arc;

fn errors() -> RuntimeErrors {
    let reference = |text: &[u8]| {
        Domain {
            kind: 0x4600_00ff,
            schema: 1,
        }
        .reference(text)
        .unwrap()
    };
    RuntimeErrors {
        resource: reference(b"resource"),
        cycle: reference(b"cycle"),
        cancelled: reference(b"cancelled"),
        protocol: reference(b"protocol"),
        panicked: reference(b"panic"),
    }
}
#[test]
fn admitted_real_front_tasks_match_scalar_modes_and_complete_reads() {
    hirsel_engine::with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(
            &path,
            "fn left()->u8{1u8}\nfn right()->u8{left()}\npub fn main()->u8{right()}\n",
        )
        .unwrap();
        for jobs in [1, 2] {
            let store = Arc::new(MemStore::default());
            let config = CompileConfig {
                store: store.clone(),
                jobs: hirsel_intern::jobs::Jobs::new(jobs),
                ..Default::default()
            };
            let mut program = COMPILER
                .with_config(config)
                .with_owned_engines()
                .load_program(&path)
                .unwrap();
            let mut definitions = vec![];
            for name in ["left", "right", "main"] {
                let mut path = program.entry.clone();
                path.0.name = name.into();
                definitions.push(path);
            }
            let budget = ProcessBudget::new(1 << 30);
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(jobs),
                budget.reserve(512 << 20).unwrap(),
                errors(),
            )
            .unwrap();
            let queries = program
                .engine
                .front_body_queries(
                    &definitions,
                    MemoryWeight {
                        retained: 1 << 20,
                        scratch: 8 << 20,
                        stack: 0,
                    },
                    budget.reserve(8 << 20).unwrap(),
                    errors(),
                )
                .unwrap();
            let roots: Vec<_> = definitions
                .iter()
                .map(|path| queries.query(path).unwrap())
                .collect();
            runtime
                .register(FRONT_BODY_KIND, queries.factory())
                .unwrap();
            let request = runtime.request(&roots);
            let outputs = runtime.wait(request).unwrap();
            assert_eq!(outputs.len(), 3);
            assert_eq!(runtime.stats().claims, 3);
            assert_eq!(runtime.stats().polls, 3);
            for (definition, product) in definitions.iter().zip(outputs) {
                let held = queries.outcome(product.record).unwrap();
                let actual = &held.outcome;
                let scalar = program.engine.mode_definition(definition).unwrap();
                assert_eq!(
                    actual.result.as_ref().unwrap().diagnostics,
                    scalar.diagnostics
                );
                assert_eq!(
                    actual.result.as_ref().unwrap().dependencies,
                    scalar.dependencies
                );
                let expected = program
                    .engine
                    .memo_reads(definition, QueryKind::Modes)
                    .unwrap();
                assert_eq!(
                    &*actual.reads, expected,
                    "complete front reads for {definition}, jobs={jobs}"
                );
                assert!(actual.summary.is_some());
                assert!(store.get_content(product.record).is_some());
            }
            runtime.shutdown();
        }
    });
}

#[test]
fn admitted_lower_saved_work_and_native_metadata_survive_dependency_suspension() {
    hirsel_engine::with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(&path, "const A:u8=comptime 1u8;\nconst B:u8=comptime A;\nfn helper()->u8{B}\npub fn main()->u8{comptime helper()}\n").unwrap();
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
            let definitions: Vec<_> = ["A", "B", "helper", "main"]
                .iter()
                .map(|name| {
                    let mut path = program.entry.clone();
                    path.0.name = (*name).into();
                    path
                })
                .collect();
            let budget = ProcessBudget::new(1 << 30);
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(jobs),
                budget.reserve(512 << 20).unwrap(),
                errors(),
            )
            .unwrap();
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
                    budget.reserve(16 << 20).unwrap(),
                    errors(),
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
                    errors(),
                )
                .unwrap();
            runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
            runtime
                .register_local(FRONT_LOWER_KIND, lower.factory())
                .unwrap();
            runtime
                .register_local(FRONT_EVALUATE_KIND, lower.factory())
                .unwrap();
            let query = lower.query(&program.entry).unwrap();
            let request = runtime.request(&[query]);
            let products = runtime
                .wait(request)
                .unwrap_or_else(|error| panic!("{:?}", lower.failure(error)));
            let metadata = lower.native_product(products[0]).unwrap();
            assert!(
                metadata.verified.is_some(),
                "preserve the actual opaque source V output"
            );
            for (path, lowerings, initializers) in [
                (&definitions[0], 0, 1),
                (&definitions[1], 0, 1),
                (&definitions[2], 1, 0),
                (&definitions[3], 1, 1),
            ] {
                let work = lower.work(path);
                assert_eq!(
                    (work.lowerings, work.initializers),
                    (lowerings, initializers),
                    "saved producer work for {path}, jobs={jobs}"
                );
                for kind in [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes] {
                    assert_eq!(front.recompute_count(path, kind), 1);
                }
            }
            let actual = lower.outcome(products[0]).unwrap();
            let scalar = program.engine.lower_to_tir(&program.entry).unwrap();
            let records = NativeRecords::new(program.engine.types().clone(), store.clone());
            assert_eq!(
                records.encode(&program.engine, &actual.output.value),
                records.encode(&program.engine, &scalar.value)
            );
            assert_eq!(actual.body.domain, NATIVE_ORIGINAL_BODY);
            let native = program
                .engine
                .native_original_context(&[program.entry.clone()])
                .unwrap()
                .reader([actual.native_product()], store.clone())
                .unwrap();
            assert!(native.read(&program.entry).is_some());
            let repeated = runtime.request(&[query]);
            assert_eq!(runtime.wait(repeated).unwrap(), products);
            assert_eq!(
                lower.work(&program.entry).initializers,
                1,
                "a second waiter executes no initializer again"
            );
            assert_eq!(
                runtime.stats().claims,
                8,
                "four P, two Lower and two Evaluate nodes"
            );
            assert!(
                runtime.stats().polls > runtime.stats().claims,
                "real dependency suspension occurred"
            );
            runtime.shutdown();
        }
    });
}
