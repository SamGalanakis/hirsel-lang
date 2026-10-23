//! Nested projected writes preserve ownership through native list builder edits.
use crate::rc_support::target;
use hirsel_driver::{compiler_adapters, native::*};
use hirsel_engine::*;

#[test]
fn nested_unique_projection_engines() {
    with_compiler_stack(|| {
        let options = Options::new(target());
        let matrix: Vec<_> = [
            Engine::Heap { campaign: true },
            Engine::Native {
                opt: OptLevel::None,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
            Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        ]
        .into_iter()
        .map(|engine| Configuration {
            options: options.clone(),
            engine,
        })
        .collect();
        let directory = tempfile::tempdir().unwrap();
        let tools = Toolchain::for_matrix(&directory.path().join("runtime"), &matrix).unwrap();
        let mut failures = vec![];
        for (name, source) in [
            (
                "literal",
                include_str!(
                    "../../../../conformance/MEM/MEM-5-nested-unique-projection-37-run.hir"
                ),
            ),
            (
                "builder",
                include_str!(
                    "../../../../conformance/MEM/MEM-5-nested-unique-projection-37-builder-run.hir"
                ),
            ),
            (
                "copy",
                include_str!(
                    "../../../../conformance/MEM/MEM-5-nested-unique-projection-37-copy-run.hir"
                ),
            ),
            (
                "minimal",
                include_str!(
                    "../../../../conformance/MEM/MEM-5-nested-unique-projection-37-minimal-run.hir"
                ),
            ),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine.sources.add("projection.hir", source.into()).unwrap();
            let diagnostics = check_snapshot(&mut engine, file, &context).unwrap();
            assert!(diagnostics.is_empty(), "{name}: {diagnostics:?}");
            let entry = entry_path(&context);
            let launch = RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let reference =
                run_observed_snapshot(&mut engine, &launch, Default::default()).unwrap();
            let reference = Observation::of_reference(reference.observation.as_ref().unwrap());
            assert_eq!(reference.exit_code, Some(0), "{name}: {reference:?}");
            let checked = run_observed_checked(&mut engine, &launch, Default::default()).unwrap();
            eprintln!("{name}: checked H: {:?}", checked.outcome);
            if checked
                .observation
                .as_ref()
                .map(Observation::of_reference)
                .as_ref()
                != Some(&reference)
            {
                failures.push(format!("{name}: {:?}", checked.outcome));
            }
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let case = directory.path().join(name);
            std::fs::create_dir_all(&case).unwrap();
            for (configuration, outcome) in
                differential(&validated, &entry, &reference, &matrix, &tools, &case)
            {
                eprintln!("{name}: {configuration}: {outcome:?}");
                assert_eq!(outcome, Outcome::Agree, "{name}: {configuration}");
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}
