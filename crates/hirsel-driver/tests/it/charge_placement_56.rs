//! CT-4 loop entries with live rebinding and incoming loan ends.
use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn loop_entry_charges_precede_rebinding_and_loan_ends_on_four_engines() {
    use hirsel_driver::native::*;
    use hirsel_tir::{InstructionKind, SourceEventKind};
    with_compiler_stack(|| {
        let source = include_str!("../../../../conformance/CT/CT-4-loop-entry-charge-56-run.hir");
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("charge-placement.hir", source.into())
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let entry = entry_path(&default_context());
        let run = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 56,
                inputs: vec![],
            },
            Default::default(),
        )
        .unwrap();
        let reference = Observation::of_reference(run.observation.as_ref().unwrap());
        assert_eq!(reference.exit_code, Some(0), "{reference:?}");
        assert!(reference.failures.is_empty(), "{reference:?}");
        let input = engine.export_native_input().unwrap();
        let mut loops = 0;
        let mut incoming_loan_ends = 0;
        for definition in input.definitions.values() {
            for body in &definition.bodies {
                for block in &body.blocks {
                    for (index, instruction) in block.instructions.iter().enumerate() {
                        if matches!(instruction.kind, InstructionKind::Charge(event)
                            if event.kind == SourceEventKind::Iteration)
                        {
                            assert_eq!(index, 0, "{}", definition.path);
                            loops += 1;
                            if matches!(
                                block.instructions.get(1).map(|i| &i.kind),
                                Some(InstructionKind::EndLoan { .. })
                            ) {
                                incoming_loan_ends += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(loops, 5, "one charge for each source loop");
        assert!(
            incoming_loan_ends > 0,
            "fixture must exercise loan ends at body entry"
        );
        let validated = engine.import_native_input(input).unwrap();
        let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = Validation::EveryStage;
        let matrix = [
            Configuration {
                options: options.clone(),
                engine: Engine::Heap { campaign: false },
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
        let scratch = tempfile::Builder::new()
            .prefix("charge-placement-56-")
            .tempdir()
            .unwrap();
        let toolchain = Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
        let outcomes = differential(
            &validated,
            &entry,
            &reference,
            &matrix,
            &toolchain,
            scratch.path(),
        );
        assert_eq!(outcomes.len(), 3);
        for (configuration, outcome) in outcomes {
            assert_eq!(outcome, Outcome::Agree, "{configuration}");
        }
    });
}
