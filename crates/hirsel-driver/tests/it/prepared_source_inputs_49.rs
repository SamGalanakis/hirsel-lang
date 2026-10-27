//! Prepared source catalogues retain exact admission and pinned source identity.
use hirsel_driver::COMPILER;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

#[test]
fn prepared_source_inputs_enforce_exact_caps_and_snapshot_identity_49() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"prepared_source","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        std::fs::write(
            &path,
            "type Token { value:u64 }\npub fn main()->u64 { Token(value:7u64).value }\n",
        )
        .unwrap();
        let load = || {
            COMPILER
                .with_config(CompileConfig {
                    store: Arc::new(MemStore::default()),
                    hold_bytes: 0,
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap()
        };
        let mut program = load();
        let mut another = load();
        let paths = program.engine.front_source_roots();
        let budget = ProcessBudget::new(128 << 20);
        let errors = crate::scheduled_front::errors();
        let weight = MemoryWeight {
            retained: 64 << 10,
            scratch: 8 << 20,
            stack: 0,
        };
        let front = program
            .engine
            .front_body_queries(&paths, weight, budget.reserve(16 << 20).unwrap(), errors)
            .unwrap();
        let lower = program
            .engine
            .front_lower_queries(
                &paths,
                front.clone(),
                weight,
                3 * paths.len(),
                budget.reserve(16 << 20).unwrap(),
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
                        .map(|(file, context)| (*file, context.clone(), LoweringPolicy::Comptime))
                        .collect(),
                    front: front.clone(),
                    lower: lower.clone(),
                    growth,
                    unsafe_ffi: program.unsafe_ffi.clone(),
                    lint_package: None,
                },
                weight,
                budget.reserve(16 << 20).unwrap(),
                errors,
            )
            .unwrap();
        let before = lower.work_snapshot();
        let inputs = program.engine.front_run_inputs(16 << 20).unwrap();
        let run_bytes = inputs.input_bytes();
        assert!(program.engine.front_run_inputs(run_bytes - 1).is_err());
        assert!(
            program
                .engine
                .front_run_queries_with_inputs(
                    check.clone(),
                    front.clone(),
                    lower.clone(),
                    inputs,
                    weight,
                    budget.reserve(run_bytes - 1).unwrap(),
                    errors
                )
                .is_err()
        );
        let inputs = another.engine.front_run_inputs(16 << 20).unwrap();
        assert!(
            program
                .engine
                .front_run_queries_with_inputs(
                    check.clone(),
                    front.clone(),
                    lower.clone(),
                    inputs,
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors
                )
                .is_err(),
            "same source from another pinned snapshot cannot replace Run inputs"
        );
        let inputs = program.engine.front_run_inputs(run_bytes).unwrap();
        let run = program
            .engine
            .front_run_queries_with_inputs(
                check,
                front.clone(),
                lower.clone(),
                inputs,
                weight,
                budget.reserve(run_bytes).unwrap(),
                errors,
            )
            .unwrap();
        assert!(run.query(&program.entry).is_some());
        let inputs = program
            .engine
            .front_native_definition_inputs(&paths, 16 << 20)
            .unwrap();
        let native_bytes = inputs.input_bytes();
        assert!(
            native_bytes > FrontNativeDefinitionQueries::input_bytes(&paths).unwrap(),
            "representation data is included in admission"
        );
        assert!(
            program
                .engine
                .front_native_definition_inputs(&paths, native_bytes - 1)
                .is_err()
        );
        assert!(
            program
                .engine
                .front_native_definition_queries_with_inputs(
                    front.clone(),
                    lower.clone(),
                    inputs,
                    weight,
                    budget.reserve(native_bytes - 1).unwrap(),
                    errors
                )
                .is_err()
        );
        let inputs = another
            .engine
            .front_native_definition_inputs(&paths, 16 << 20)
            .unwrap();
        assert!(
            program
                .engine
                .front_native_definition_queries_with_inputs(
                    front.clone(),
                    lower.clone(),
                    inputs,
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors
                )
                .is_err(),
            "same source from another pinned snapshot cannot replace native inputs"
        );
        let inputs = program
            .engine
            .front_native_definition_inputs(&paths, native_bytes)
            .unwrap();
        let native = program
            .engine
            .front_native_definition_queries_with_inputs(
                front,
                lower.clone(),
                inputs,
                weight,
                budget.reserve(native_bytes).unwrap(),
                errors,
            )
            .unwrap();
        assert!(native.query(&program.entry).is_some());
        assert_eq!(
            lower.work_snapshot(),
            before,
            "catalogue preparation and installation never run Lower"
        );
        drop(native);
        drop(run);
        drop(lower);
        assert_eq!(budget.reserved(), 0);
    });
}

#[test]
fn semantic_source_products_do_not_require_a_launch_or_program_check() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"semantic_library","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[]}"#).unwrap();
        let mut source = (0..32)
            .map(|n| format!("pub fn value_{n}()->u64 {{{n}u64}}\n"))
            .collect::<String>();
        source = source.replace(
            "pub fn value_0()->u64 {0u64}",
            "pub fn value_0()->u64 {ANSWER}",
        );
        source.push_str("pub fn square(n:u64)->u64 {n*n}\nconst ANSWER:u64=comptime square(7u64)\nfn invalid()->u64 {true}\n");
        std::fs::write(directory.path().join("main.hir"), source).unwrap();
        let mut program = COMPILER
            .with_config(CompileConfig {
                store: Arc::new(MemStore::default()),
                hold_bytes: 0,
                ..Default::default()
            })
            .with_owned_engines()
            .load_check_package(&manifest)
            .unwrap();
        let paths = program.engine.front_source_roots();
        let budget = ProcessBudget::new(256 << 20);
        let errors = crate::scheduled_front::errors();
        let weight = MemoryWeight {
            retained: 64 << 10,
            scratch: 8 << 20,
            stack: 0,
        };
        let front = program
            .engine
            .front_body_queries(&paths, weight, budget.reserve(16 << 20).unwrap(), errors)
            .unwrap();
        let lower = program
            .engine
            .front_lower_queries(
                &paths,
                front.clone(),
                weight,
                3 * paths.len(),
                budget.reserve(16 << 20).unwrap(),
                errors,
            )
            .unwrap();
        let run = program
            .engine
            .front_semantic_queries(
                front.clone(),
                lower.clone(),
                MemoryWeight {
                    retained: 8 << 20,
                    ..weight
                },
                budget.reserve(16 << 20).unwrap(),
                errors,
            )
            .unwrap();
        let path = paths
            .iter()
            .find(|path| path.0.name == "value_0")
            .unwrap()
            .clone();
        let query = run.semantic_query(&path).unwrap();
        assert!(run.query(&path).is_none());
        let expected = program
            .engine
            .semantic_program_nodes([path.clone()])
            .unwrap()
            .into_iter()
            .find(|node| node.identity == path.to_string())
            .unwrap();
        let expected_audit = program.engine.source_audit(&path).unwrap();
        let answer = paths
            .iter()
            .find(|path| path.0.name == "ANSWER")
            .unwrap()
            .clone();
        let expected_constant = program.engine.materialize_value(&answer).unwrap();
        let queries = paths
            .iter()
            .filter(|path| path.0.name.starts_with("value_"))
            .map(|path| run.semantic_query(path).unwrap())
            .collect::<Vec<_>>();
        drop(program);
        let mut runtime = AdmittedRuntime::new(
            hirsel_intern::jobs::Jobs::new(2),
            budget.reserve(192 << 20).unwrap(),
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
        runtime.register(FRONT_RUN_KIND, run.factory()).unwrap();
        // No Check or Growth factory is installed. The invalid sibling must
        // remain undemanded, and the library has no launch entry.
        let request = runtime.request(&[query]);
        let product = runtime.wait(request).unwrap()[0];
        let actual = run.semantic_node(product).unwrap();
        assert_eq!(actual.identity, expected.identity);
        assert_eq!(actual.content, expected.content);
        assert_eq!(actual.dependencies, expected.dependencies);
        assert!(run.outcome(product).is_none());
        let bytes = run.source_product_bytes(product).unwrap().unwrap();
        assert!(
            run.source_products_with_allowance(product, budget.reserve(bytes - 1).unwrap())
                .is_err()
        );
        let products = run
            .source_products_with_allowance(product, budget.reserve(bytes).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(products.audit(), &expected_audit);
        assert!(products.constant().is_none());
        drop(products);
        let request = runtime.request(&[run.semantic_query(&answer).unwrap()]);
        let answer_product = runtime.wait(request).unwrap()[0];
        let bytes = run.source_product_bytes(answer_product).unwrap().unwrap();
        let products = run
            .source_products_with_allowance(answer_product, budget.reserve(bytes).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            format!("{:?}", products.constant().unwrap()),
            format!("{expected_constant:?}")
        );
        drop(products);
        // Their 32 four-MiB continuation grants cannot all fit this existing
        // 64-MiB working grant plus its existing 128-MiB worker stack. Completed node outputs have their own reservation.
        for query in queries {
            let request = runtime.request(&[query]);
            let product = runtime.wait(request).unwrap()[0];
            assert!(run.semantic_node(product).is_ok());
        }
        let before = lower.work_snapshot();
        let repeated = runtime.request(&[query]);
        assert_eq!(runtime.wait(repeated).unwrap(), vec![product]);
        assert_eq!(lower.work_snapshot(), before);
        let mut wrong = product;
        wrong.action = hirsel_artifact_store::Action(hirsel_syntax::ContentHash::default());
        assert!(run.semantic_node(wrong).is_err());
        runtime.shutdown();
    });
}
