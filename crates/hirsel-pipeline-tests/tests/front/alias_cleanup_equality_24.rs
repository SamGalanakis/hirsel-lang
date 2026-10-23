//! Bounded nominal callback results survive success and failure on every engine.
use hirsel_driver::native::*;
use hirsel_engine::{
    QueryEngine, RunConfiguration, check_snapshot, default_context, default_environment,
    entry_path, run_observed_snapshot, with_compiler_stack,
};

#[test]
fn alias_cleanup_equality_24_reference_h_llvm_cranelift() {
    with_compiler_stack(|| {
        let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = Validation::EveryStage;
        let matrix = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Heap { campaign: true },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Speed,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options,
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let directory =
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("alias-cleanup-equality-24");
        let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
        let mut agreements = 0;
        let mut references = 0;
        for (name, source) in [
            (
                "direct",
                include_str!("../../../../conformance/MEM/MEM-5-alias-cleanup-24-direct-run.hir"),
            ),
            (
                "transparent",
                include_str!(
                    "../../../../conformance/MEM/MEM-5-alias-cleanup-24-transparent-run.hir"
                ),
            ),
            (
                "nominal",
                include_str!("../../../../conformance/MEM/MEM-5-alias-cleanup-24-nominal-run.hir"),
            ),
            (
                "nested",
                include_str!("../../../../conformance/MEM/MEM-5-alias-cleanup-24-nested-run.hir"),
            ),
            (
                "swapped",
                include_str!("../../../../conformance/MEM/MEM-5-alias-cleanup-24-swapped-run.hir"),
            ),
        ] {
            for failure in [false, true] {
                let source = if failure {
                    source
                        .replace("relay(one, 1u64)", "relay(one, 0u64)")
                        .replace("relay(one,1u64)", "relay(one,0u64)")
                } else {
                    source.into()
                };
                let mut engine =
                    QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
                let file = engine
                    .sources
                    .add("alias-cleanup.hir", source.clone())
                    .unwrap();
                let ds = check_snapshot(&mut engine, file, &default_context()).unwrap();
                assert!(ds.is_empty(), "{source}\n{ds:?}");
                let entry = entry_path(&default_context());
                let run = run_observed_snapshot(
                    &mut engine,
                    &RunConfiguration {
                        entry: entry.clone(),
                        hash_seed: 0,
                        inputs: vec![],
                    },
                    Default::default(),
                )
                .unwrap();
                let reference = Observation::of_reference(run.observation.as_ref().unwrap());
                assert!(reference.output.is_empty());
                if failure {
                    assert_eq!(reference.exit_code, None);
                    assert_eq!(reference.failures.len(), 1);
                    assert_eq!(reference.failures[0].rule, "SEM-1");
                } else {
                    assert_eq!(reference.exit_code, Some(0));
                    assert!(reference.failures.is_empty());
                }
                references += 1;
                let input = engine.export_native_input().unwrap();
                let input = engine.import_native_input(input).unwrap();
                let case = directory.join(format!("{name}-{failure}"));
                std::fs::create_dir_all(&case).unwrap();
                let outcomes = differential(&input, &entry, &reference, &matrix, &toolchain, &case);
                assert_eq!(outcomes.len(), 3);
                for (configuration, outcome) in outcomes {
                    assert!(
                        matches!(outcome, Outcome::Agree),
                        "{name}/{failure} {configuration}: {outcome:?}"
                    );
                    agreements += 1;
                }
            }
        }
        assert_eq!(references, 10);
        assert_eq!(agreements, 30);
        eprintln!(
            "alias_cleanup_equality_24: {references} interpreter outcomes, {agreements} H/LLVM/Cranelift agreements"
        );
    });
}
