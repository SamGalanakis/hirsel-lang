//! Executable NetPeer repair recipe. Payload equality is not a lifetime proof.
use hirsel_driver::{load_program, native};
use hirsel_engine::{LoweringPolicy, RunConfiguration, SimulationConfig, with_compiler_stack};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn ownership_repairs_reject_original_and_paired_misuses() {
    with_compiler_stack(|| {
        for (name, rule, primary) in [
            ("original", "MODE-1", "[Open([Send(data), Close()])]"),
            ("view-live", "MODE-1", "data = b\"changed\""),
            ("view-escape", "MODE-1", "peer(data)"),
            ("take-reuse", "OWN-2", "data"),
            ("copy-still-borrows", "MODE-1", "peer(copy data)"),
            ("take-borrowed", "OWN-1", "peer(take data)"),
        ] {
            let path = root().join(format!(
                "crates/hirsel-driver/tests/fixtures/ownership-repairs-6/{name}.hir"
            ));
            let mut program = load_program(&path).unwrap();
            let output = program.check(LoweringPolicy::Comptime).unwrap();
            let errors: Vec<_> = output
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            assert_eq!(errors.len(), 1, "{name}: {errors:?}");
            assert_eq!(output.completion, hirsel_engine::CheckCompletion::Complete);
            assert_eq!(errors[0].rule.as_str(), rule, "{name}: {errors:?}");
            let at = output.sources.resolve(errors[0].primary);
            let source = std::fs::read_to_string(&path).unwrap();
            let start = source.rfind(primary).unwrap() as u32;
            assert_eq!(
                (at.start, at.end),
                (start, start + primary.len() as u32),
                "{name}"
            );
            if name == "original" || name == "copy-still-borrows" {
                let notes = errors[0].notes.join("\n");
                for meaning in [
                    "declare the parameter `take data`",
                    "callers write `take data`",
                    "moves their value",
                    "`unique data`",
                    "deep copy",
                    "O(size)",
                    "`copy data` does not help",
                    "stay local",
                ] {
                    assert!(
                        notes.contains(meaning),
                        "{name}: missing {meaning}: {notes}"
                    );
                }
                if name == "original" {
                    let provided = output.sources.resolve(errors[0].secondary[0].span);
                    let parameter_type = source.find("Bytes").unwrap() as u32;
                    assert_eq!(
                        (provided.start, provided.end),
                        (parameter_type, parameter_type + 5)
                    );
                }
            }
        }
    });
}

#[test]
fn ownership_repairs_payloads_and_lifetimes_agree_on_h_llvm_and_cranelift() {
    with_compiler_stack(|| {
        let mut program =
            load_program(&root().join("examples/ownership-repairs/hirsel.json")).unwrap();
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
        assert!(
            observation.effects.is_empty(),
            "fixture must have no capability effects"
        );
        let reference = native::Observation::of_reference(observation);
        assert_eq!(reference.exit_code, Some(0), "{reference:?}");
        assert!(reference.failures.is_empty());
        let input = program.engine.export_native_input().unwrap();
        let validated = program.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let configurations = vec![
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Heap { campaign: false },
            },
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Native {
                    opt: native::OptLevel::Speed,
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
            native::Configuration {
                options,
                engine: native::Engine::Cranelift {
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
        ];
        assert_eq!(configurations.len(), 3);
        let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ownership-repairs-6");
        std::fs::create_dir_all(&scratch).unwrap();
        let toolchain =
            native::Toolchain::for_matrix(&scratch.join("runtime"), &configurations).unwrap();
        let outcomes = native::differential(
            &validated,
            &program.entry,
            &reference,
            &configurations,
            &toolchain,
            &scratch,
        );
        assert_eq!(outcomes.len(), 3);
        for (configuration, outcome) in outcomes {
            eprintln!("ownership-repairs-6: {configuration}: {outcome:?}");
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{}: {outcome:?}",
                configuration
            );
        }
    });
}
