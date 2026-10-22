//! Different string cursors must compose in one native descriptor registry.
use crate::rc_support::target;

const EMPTY: &str = r#"
pub fn main() -> Result[(), ()] {
    let source = "a"
    for ch in string.scalars(source) { () }
    for p in string.positions(source) { () }
    Ok(())
}
"#;

const CHECKED: &str = r#"
pub fn main() -> Result[(), ()] {
    let source = "é"
    for ch in string.scalars(source) {
        failure.check(ch == 'é', TextInvariant())
    }
    for p in string.positions(source) {
        failure.check(string.byte_offset(source, p) <= 2, TextInvariant())
    }
    Ok(())
}
"#;

#[test]
#[ignore = "native: string cursor descriptors on H, checked heap, LLVM and Cranelift"]
fn both_string_cursors_have_consistent_descriptors() {
    native_parity(&[("empty", EMPTY), ("checked", CHECKED)]);
}

fn native_parity(cases: &[(&str, &str)]) {
    use hirsel_driver::native::*;
    use hirsel_driver::*;
    use hirsel_engine::*;
    let options = Options::new(target());
    let matrix = vec![
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
    let directory =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native-desc-cell-16");
    std::fs::create_dir_all(&directory).unwrap();
    let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
    for &(name, source) in cases {
        with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine.sources.add("cursors.hir", source.into()).unwrap();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
            let entry = entry_path(&context);
            let reference = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let reference = Observation::of_reference(reference.observation.as_ref().unwrap());
            assert_eq!(reference.exit_code, Some(0), "{name}: {reference:?}");
            assert!(reference.failures.is_empty(), "{name}: {reference:?}");
            eprintln!("{name}: interpreter exit0");
            let snapshot = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(snapshot).unwrap();
            let directory = directory.join(name);
            std::fs::create_dir_all(&directory).unwrap();
            let mut executions = 0;
            for (configuration, outcome) in differential(
                &validated, &entry, &reference, &matrix, &toolchain, &directory,
            ) {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name}: {configuration}: {outcome:?}"
                );
                eprintln!("{name}: {configuration}: agrees");
                executions += 1;
            }
            assert_eq!(executions, 3);
        });
    }
}
