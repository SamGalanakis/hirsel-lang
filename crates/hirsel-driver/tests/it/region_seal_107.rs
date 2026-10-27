use hirsel_driver::{load_program, native};
use hirsel_engine::{RunConfiguration, SimulationConfig, with_compiler_stack};

#[test]
fn sealed_heap_107_runs_on_four_engines_and_replays() {
    run_sealed_program("MEM-4-sealed-heap-run.hir");
}

#[test]
fn sealed_heap_charge_107_runs_on_four_engines_and_replays() {
    run_sealed_program("MEM-4-sealed-heap-charge-run.hir");
}

#[test]
fn sealed_empty_heap_107_runs_on_four_engines_and_replays() {
    run_sealed_program("MEM-4-sealed-empty-heap-empty-run.hir");
}

#[test]
fn sealed_arena_root_117_runs_on_four_engines_and_replays() {
    run_sealed_program("MEM-4-sealed-arena-root-run.hir");
}

fn run_sealed_program(name: &str) {
    with_compiler_stack(|| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/MEM")
            .join(name);
        let mut program = load_program(&path).unwrap();
        program.check_for_execution().unwrap();
        let run = RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let reference = hirsel_engine::run_observed_snapshot(
            &mut program.engine,
            &run,
            SimulationConfig::default(),
        )
        .unwrap();
        let observation =
            native::Observation::of_reference(reference.observation.as_ref().unwrap());
        assert_eq!(observation.exit_code, Some(0));
        assert!(observation.failures.is_empty());
        let mut matrix = native::trace_roots_matrix();
        matrix.push(native::Configuration {
            options: native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            engine: native::Engine::Heap { campaign: false },
        });
        let scratch = tempfile::tempdir().unwrap();
        let tools =
            native::Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
        let input = program.engine.export_native_input().unwrap();
        let validated = program.engine.import_native_input(input).unwrap();
        let outcomes = native::differential_in(
            &validated,
            &run.entry,
            &observation,
            &matrix,
            &tools,
            scratch.path(),
            &native::Fixture::default(),
        );
        assert_eq!(outcomes.len(), 3);
        let mut failures = Vec::new();
        for (configuration, outcome) in outcomes {
            eprintln!("{configuration}: {outcome:?}");
            if !matches!(outcome, native::Outcome::Agree) {
                failures.push((configuration, outcome));
            }
        }
        let replay = native::real_io_replay_in(
            &validated,
            &run.entry,
            &observation,
            &tools,
            scratch.path(),
            Default::default(),
            &native::Fixture::default(),
        );
        assert!(failures.is_empty(), "{failures:#?}; replay: {replay:?}");
        assert!(
            matches!(replay, native::Outcome::Agree),
            "replay: {replay:?}"
        );
    });
}

#[test]
fn sealed_heap_107_snapshot_keeps_verifier_outputs_without_an_ordinary_store() {
    with_compiler_stack(|| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/MEM/MEM-4-sealed-heap-run.hir");
        let mut program = load_program(&path).unwrap();
        program.check_for_execution().unwrap();
        let input = program.engine.export_native_input().unwrap();
        let validated = program.engine.import_native_input(input).unwrap();
        let reader = validated.original_reader().unwrap();
        for path in validated.snapshot().definitions.keys() {
            let original = reader.read(path).expect("original selected source");
            let output = original.output().expect("actual verifier output");
            assert_eq!(output.owner().path(), path);
            assert_eq!(
                original.definition(),
                &validated.snapshot().definitions[path]
            );
            assert_eq!(reader.input(path).unwrap().0.unwrap(), output);
        }
    });
}

#[test]
fn sealed_heap_107_original_graph_names_its_seal_and_capture_free_callbacks() {
    with_compiler_stack(|| {
        use hirsel_driver::native::receipt_source::{
            factory::{NamedInput, named_graph},
            provider::Front,
        };
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/MEM/MEM-4-sealed-heap-run.hir");
        let mut program = load_program(&path).unwrap();
        program.check_for_execution().unwrap();
        let input = program.engine.export_native_input().unwrap();
        let validated = program.engine.import_native_input(input).unwrap();
        let input = validated.snapshot();
        let original = &input.definitions[&program.entry];
        let base = hirsel_layout::instance::InstanceKey::new(
            &program.entry,
            original.entry.0,
            &[],
            &input.types,
            None,
            None,
        )
        .unwrap();
        let provider = Front::new(validated.original_reader().unwrap());
        let graph = named_graph(
            &provider,
            [NamedInput {
                base,
                actuals: hirsel_rc::instance::Actuals::none(),
            }],
            &mut |_, _, _| panic!("this program has no held callable boundary"),
        )
        .unwrap();
        assert_eq!(
            graph
                .operations
                .iter()
                .filter(|op| op.operation.definition.is_core("arena.seal"))
                .count(),
            1
        );
        assert_eq!(
            graph
                .operations
                .iter()
                .filter(|op| op.operation.definition.is_core("region.with"))
                .count(),
            2
        );
        assert_eq!(graph.closures.len(), 3);
        assert!(
            graph
                .contexts
                .iter()
                .all(|context| context.source.entry().captures().is_empty())
        );
    });
}
