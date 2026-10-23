//! Compile and execute the published Markdown, with handwritten value oracles.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

const RECIPES: &str = include_str!("../../../../docs/std-api-recipes.md");

fn block(marker: &str, language: &str) -> &'static str {
    let tail = RECIPES
        .split_once(marker)
        .expect("published recipe marker")
        .1;
    let code = tail
        .split_once(&format!("```{language}\n"))
        .expect("published recipe fence")
        .1;
    code.split_once("\n```").expect("closing fence").0
}

fn recipe(name: &str) {
    with_compiler_stack(|| {
        let scratch = tempfile::Builder::new()
            .prefix("std-api-recipes-24-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_str(block("<!-- recipe-manifest -->", "json")).unwrap();
        // Relocate only the selected std dependency; source and entry are published inputs.
        let std_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        manifest["dependencies"][0]["manifest"] =
            serde_json::Value::String(std_manifest.to_str().unwrap().into());
        let selection = scratch.path().join("hirsel.json");
        std::fs::write(&selection, serde_json::to_vec(&manifest).unwrap()).unwrap();
        std::fs::write(
            scratch.path().join("main.hir"),
            block(&format!("<!-- recipe:{name} -->"), "hirsel"),
        )
        .unwrap();
        let mut loaded = load_program(&selection).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| match error {
                ToolError::Query { error, .. } => panic!("{name}: {error:?}"),
                error => panic!("{name}: {error}"),
            });
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(1_000_000),
                call_depth: 256,
            },
            ..Default::default()
        }));
        let reference = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                reference.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{name}: {:?}",
            reference.outcome
        );
        let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        assert_eq!(expected.exit_code, Some(0));
        assert!(expected.output.is_empty());
        assert!(expected.failures.is_empty());
        // Reachable export/import independently validates the execution input.
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let sites = native::Sites::new(validated.snapshot());
        let heap = native::run_heap_in(
            &ready,
            &loaded.entry,
            &sites,
            &native::HeapWorld {
                steps: Some(1_000_000),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(heap.observation, expected, "{name}");
        assert_eq!(heap.counters.live_heap_bytes, 0, "{name}");
        eprintln!(
            "{name}: published source checked; reachable input validated; interpreter + checked H passed"
        );
    });
}

#[test]
fn object_member() {
    recipe("object");
}

#[test]
fn array_element() {
    recipe("array");
}

#[test]
fn json_text_escaping() {
    recipe("json-text");
}

#[test]
fn owned_valid_utf8() {
    recipe("utf8-valid");
}

#[test]
fn owned_invalid_utf8_preserves_bytes() {
    recipe("utf8-invalid");
}

#[test]
fn supplied_civil_time() {
    recipe("time");
}
