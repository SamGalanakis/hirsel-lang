//! Checked record, binding, literal and capability-error repairs.
use hirsel_driver::{load_program, native};
use hirsel_engine::{LoweringPolicy, RunConfiguration, SimulationConfig, with_compiler_stack};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn authoring_repairs_11_rejected_twins_have_one_intended_rule_and_span() {
    with_compiler_stack(|| {
        for (friction, name, rule, primary) in [
            ("dogfood7-F02", "path-copy", "MODE-1", "copy asset.path"),
            ("dogfood7-F02", "partial-move", "OWN-8", "take value.assets"),
            ("dogfood7-F02", "record-reuse", "OWN-2", "asset.path"),
            ("dogfood7-F02", "field-reuse", "OWN-2", "name"),
            ("dogfood7-F03", "mutable-pattern", "LANG-13", "mut"),
            (
                "dogfood7-F04",
                "sequence-context",
                "LANG-2",
                "[\"a\", \"b\"]",
            ),
            ("dogfood7-F05", "file-error", "LANG-8", "?"),
            ("dogfood7-F05", "grant-error", "LANG-8", "?"),
            ("dogfood8-F02", "unique-take", "MODE-2", "Ok(take config)"),
            ("dogfood8-F03", "byte-literal", "SEM-3", "雪"),
            ("dogfood8-F04", "numeric-context", "LANG-2", "65537"),
        ] {
            let path = root().join(format!(
                "crates/hirsel-driver/tests/fixtures/authoring-repairs-11/{name}.hir"
            ));
            let (diagnostics, sources) = match load_program(&path) {
                Ok(mut program) => {
                    let output = program.check(LoweringPolicy::Comptime).unwrap();
                    assert_eq!(output.completion, hirsel_engine::CheckCompletion::Complete);
                    (output.diagnostics, output.sources)
                }
                // Syntax/literal errors can be returned before a program is loaded.
                Err(hirsel_engine::ToolError::Query {
                    error: hirsel_engine::QueryError::Diagnostics(diagnostics),
                    sources,
                    ..
                }) => {
                    assert!(matches!(name, "mutable-pattern" | "byte-literal"));
                    (diagnostics, sources)
                }
                Err(error) => panic!("{name}: {error:?}"),
            };
            let errors: Vec<_> = diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            assert_eq!(errors.len(), 1, "{name}: {errors:?}");
            assert_eq!(errors[0].rule.as_str(), rule, "{name}: {errors:?}");
            let at = sources.resolve(errors[0].primary);
            let source = std::fs::read_to_string(&path).unwrap();
            // Anchor the read rather than a declaration of the same field binding.
            let start = if name == "field-reuse" {
                source.find("let _ = name").unwrap() + "let _ = ".len()
            } else if name == "mutable-pattern" {
                source.find("mut f").unwrap()
            } else {
                source.rfind(primary).unwrap()
            } as u32;
            assert_eq!(
                (at.start, at.end),
                (start, start + primary.len() as u32),
                "{name}"
            );
            eprintln!(
                "{friction}: reject={name} count=1 rule={rule} span={}..{}",
                at.start, at.end
            );
        }
    });
}

#[test]
fn authoring_repairs_11_accepted_twins_agree_on_h_llvm_and_cranelift() {
    with_compiler_stack(|| {
        for (name, manifest) in [
            ("pure", "authoring.hir.json"),
            ("io", "authoring-io.hir.json"),
        ] {
            let mut program =
                load_program(&root().join("examples/ownership-repairs").join(manifest)).unwrap();
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
            if name == "pure" {
                assert!(observation.effects.is_empty());
            } else {
                assert!(
                    !observation.effects.is_empty(),
                    "I/O twin must exercise real operations"
                );
            }
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
            let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
                .join("authoring-repairs-11")
                .join(name);
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
                eprintln!("authoring-repairs-11/{name}: {configuration}: {outcome:?}");
                assert!(
                    matches!(outcome, native::Outcome::Agree),
                    "{}: {outcome:?}",
                    configuration
                );
            }
        }
        for (friction, pure, io) in [
            ("dogfood7-F02", 4, 0),
            ("dogfood7-F03", 1, 1),
            ("dogfood7-F04", 1, 0),
            ("dogfood7-F05", 2, 2),
            ("dogfood8-F02", 1, 0),
            ("dogfood8-F03", 1, 0),
            ("dogfood8-F04", 1, 0),
        ] {
            eprintln!("{friction}: accepted pure={pure} io={io}; checked+H+LLVM+Cranelift agree");
        }
    });
}
