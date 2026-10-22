//! One bounded fixture, compiled once per backend, with audited exits.
use hirsel_driver::native::*;
use hirsel_engine::{
    QueryEngine, RunConfiguration, check_snapshot, default_context, default_environment,
    entry_path, run_observed_snapshot, with_compiler_stack,
};

#[test]
fn bounded_loop_matrix_reference_heap_llvm_cranelift() {
    with_compiler_stack(|| {
        let source = include_str!("../../../../conformance/OWN/OWN-2-map-loop-validity-17-run.hir");
        let mut engine =
            QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
        let file = engine.sources.add("map-loop.hir", source.into()).unwrap();
        let diagnostics = check_snapshot(&mut engine, file, &default_context()).unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let entry = entry_path(&default_context());
        let execution = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            Default::default(),
        )
        .unwrap();
        let reference = Observation::of_reference(execution.observation.as_ref().unwrap());
        assert_eq!(reference.output, b"ok\n");
        assert_eq!(reference.exit_code, Some(0));
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = Validation::EveryStage;
        let root =
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("map-loop-validity-17");
        let configurations = [
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Speed,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let tools = Toolchain::for_matrix(&root.join("runtime"), &configurations).unwrap();
        let ready = build(&validated, &options).unwrap();
        let sites = Sites::new(validated.snapshot());
        let heap = run_heap(&ready, &entry, &sites).unwrap();
        assert_eq!(heap.observation, reference);
        assert_eq!(heap.counters.live_heap_bytes, 0);
        std::fs::create_dir_all(root.join("native")).unwrap();
        let outcomes = differential(
            &validated,
            &entry,
            &reference,
            &configurations,
            &tools,
            &root.join("native"),
        );
        assert_eq!(outcomes.len(), 2);
        for (configuration, outcome) in outcomes {
            assert_eq!(outcome, Outcome::Agree, "{configuration}");
        }
    });
}
