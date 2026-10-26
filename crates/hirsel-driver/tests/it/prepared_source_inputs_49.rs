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
