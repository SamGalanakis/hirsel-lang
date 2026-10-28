//! Checked cookbook: explicit nested hooks, strict wire errors and owned results.
use hirsel_driver::{
    load_program_source,
    requests::{CompileOperation, CompileResult},
    session::Session,
};
use hirsel_engine::{
    LoweringPolicy, QueryError, RunConfiguration, SimulationConfig, ToolError, with_compiler_stack,
};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn run_recipe(entry: &str) {
    with_compiler_stack(|| {
        // A public wrapper selects one check without changing the cookbook source.
        let source = format!(
            "{}\npub fn recipe_entry() -> Result[(), String] uses {{}} {{ {entry}() }}\n",
            include_str!("../../../../examples/schema-recipes/main.hir")
        );
        let mut program = load_program_source(
            &root().join("crates/hirsel-driver/tests/fixtures/std_numeric.hir"),
            Some(&source),
        )
        .unwrap();
        program.entry.0.name = "recipe_entry".into();
        program.check_for_execution().unwrap();
        let config = RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let execution = hirsel_engine::run_observed_checked(
            &mut program.engine,
            &config,
            SimulationConfig::default(),
        )
        .unwrap();
        let observation = execution.observation.as_ref().unwrap();
        assert!(observation.effects.is_empty(), "{entry}: {observation:?}");
        let actual = hirsel_driver::native::Observation::of_reference(observation);
        assert_eq!(actual.exit_code, Some(0), "{entry}: {actual:?}");
        assert!(actual.failures.is_empty(), "{entry}: {actual:?}");
    });
}

#[test]
fn schema_recipe_11_roundtrip_and_owned_return_goldens() {
    run_recipe("roundtrip");
}

#[test]
fn schema_recipe_11_strict_errors_and_duplicate_keys() {
    run_recipe("strict_errors");
}

#[test]
fn schema_recipe_11_depth_and_work_limits_in_both_directions() {
    run_recipe("limits");
}

fn reject_missing(direction: &str) {
    with_compiler_stack(|| {
        let path = root().join(format!(
            "crates/hirsel-driver/tests/fixtures/schema-recipe-11/missing-{direction}.hir"
        ));
        let mut session = Session::open(&path, hirsel_engine::Verify::On);
        let (diagnostics, sources) = match session.request(CompileOperation::Check {
            lowering: LoweringPolicy::Comptime,
        }) {
            Ok(completed) => {
                let CompileResult::Check(output) = &completed.result else {
                    panic!("expected source Check");
                };
                assert_eq!(output.completion, hirsel_engine::CheckCompletion::Complete);
                (output.diagnostics.clone(), output.sources.clone())
            }
            Err(ToolError::Query {
                error: QueryError::Diagnostics(diagnostics),
                sources,
                ..
            }) => (diagnostics, sources),
            Err(error) => panic!("expected completed language rejection: {error}"),
        };
        let errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.severity == hirsel_diag::Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "missing hooks must reject");
        assert_eq!(errors.len(), 2, "one unresolved hook per nested type");
        let mut missing = Vec::new();
        for error in errors {
            assert_eq!(error.rule.as_str(), "LANG-4", "{error:?}");
            let primary = sources.slice(error.primary).unwrap();
            missing.push(primary.to_owned());
            let origin =
                format!("comptime derive.schema_{direction}(shape(Outer), schema.strict_policy())");
            assert!(
                error.secondary.iter().any(|label| {
                    sources.slice(label.span) == Some(origin.as_str())
                        && sources
                            .file(sources.resolve(label.span).file)
                            .is_some_and(|file| file.path == path)
                }),
                "missing original comptime source span: {error:?}"
            );
        }
        missing.sort();
        assert_eq!(
            missing,
            [
                format!("schema_recipe_reject@0/retry_schema_{direction}"),
                format!("schema_recipe_reject@0/service_schema_{direction}"),
            ]
        );
    });
}

#[test]
fn schema_recipe_11_missing_decode_hook_rejects() {
    reject_missing("decode");
}

#[test]
fn schema_recipe_11_missing_encode_hook_rejects() {
    reject_missing("encode");
}
