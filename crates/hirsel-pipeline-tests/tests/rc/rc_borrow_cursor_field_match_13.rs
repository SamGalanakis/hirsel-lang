//! Field and enum payload descent must keep the cursor's borrowed owner.
use crate::rc_support::*;

const MATCH: &str =
    include_str!("../../../../conformance/LANG/LANG-15-borrow-cursor-field-match-ok.hir");
const LOOP: &str =
    include_str!("../../../../conformance/LANG/LANG-15-borrow-cursor-field-loop-ok.hir");
const NESTED: &str =
    include_str!("../../../../conformance/LANG/LANG-15-borrow-cursor-nested-field-match-ok.hir");
const MAIN: &str = r#"
pub fn main() -> Result[(), u64] {
    let leaf = box Node(value: 7, left: None())
    let a = walk(leaf)
    let branch = box Node(value: 1, left: Some(box Node(value: 7, left: None())))
    let b = walk(branch)
    let deep = box Node(value: 0, left: Some(box Node(value: 1, left: Some(box Node(value: 7, left: None())))))
    let c = walk(deep)
    if a == 7 && b == 7 && c == EXPECTED { Ok(()) } else { Error(a + b + c) }
}
"#;
fn cases() -> Vec<(&'static str, String)> {
    vec![
        (
            "field-match",
            format!("{MATCH}{}", MAIN.replace("EXPECTED", "1")),
        ),
        (
            "field-loop",
            format!("{LOOP}{}", MAIN.replace("EXPECTED", "7")),
        ),
        (
            "nested-field-loop",
            format!(
                "{NESTED}{}",
                r#"
pub fn main() -> Result[(), u64] {
    let a = walk(box Node(value: 7, kids: Kids(first: None())))
    let b = walk(box Node(value: 1, kids: Kids(first: Some(box Node(value: 7, kids: Kids(first: None()))))))
    let c = walk(box Node(value: 0, kids: Kids(first: Some(box Node(value: 1, kids: Kids(first: Some(box Node(value: 7, kids: Kids(first: None())))))))))
    if a == 7 && b == 7 && c == 7 { Ok(()) } else { Error(a + b + c) }
}
"#
            ),
        ),
    ]
}

#[test]
fn field_match_cursor_pipeline_and_zero_rc() {
    use hirsel_lir::ir::Inst;
    for (name, source) in cases() {
        let (_, lowered) = lower_valid(&source);
        let walk = unit_named(&lowered, "walk");
        assert!(
            !walk.instructions.iter().any(|instruction| matches!(
                instruction,
                Inst::Dup { .. } | Inst::Release { .. } | Inst::Copy { .. }
            )),
            "{name}: cursor descent acquired RC operations: {}",
            dump(walk)
        );
    }
}

#[test]
#[ignore = "native: field-match cursor interpreter, LLVM and Cranelift parity"]
fn field_match_cursor_native_parity() {
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
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("field-match-cursor-13");
    std::fs::create_dir_all(&directory).unwrap();
    let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
    for (name, source) in cases() {
        with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine
                .sources
                .add("field-match-cursor.hir", source)
                .unwrap();
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
            let snapshot = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(snapshot).unwrap();
            let directory = directory.join(name);
            std::fs::create_dir_all(&directory).unwrap();
            for (configuration, outcome) in differential(
                &validated, &entry, &reference, &matrix, &toolchain, &directory,
            ) {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name}: {configuration}: {outcome:?}"
                );
            }
        });
    }
}
