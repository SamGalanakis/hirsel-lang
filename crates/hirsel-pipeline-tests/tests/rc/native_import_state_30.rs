//! Native import must tolerate paths removed by its own proven tag tests.
use crate::rc_support::*;

const WITNESS: &str = r#"
type Choice = First(String) | Second(String)
fn value(x: Choice) -> u64 {
  match x {
    First(n) | Second(n) => match x { First(_) => 1, _ => 2 },
  }
}
pub fn main() -> Result[(), ()] {
  if value(First("a")) == 1 && value(Second("b")) == 2 { Ok(()) } else { Error(()) }
}
"#;

fn cases() -> Vec<(&'static str, String)> {
    vec![
        ("witness", WITNESS.into()),
        (
            "guarded",
            WITNESS.replace("First(_) => 1", "First(_) if false => 99, First(_) => 1"),
        ),
        (
            "loop",
            WITNESS
                .replace(
                    "  match x {",
                    "  let mut score = 0u64\n  for i in 0u64..2u64 {\n  score += match x {",
                )
                .replace("\n}\npub fn main", "\n  }\n  score\n}\npub fn main")
                .replace("== 1 &&", "== 2 &&")
                .replace("== 2 {", "== 4 {"),
        ),
        (
            "dead-allocation",
            WITNESS.replace("_ => 2", "_ => { let text = unique n; 2 }"),
        ),
    ]
}

#[test]
fn proven_match_prunes_incoming_state() {
    for (name, source) in cases() {
        let (_, lowered) = lower_valid(&source);
        assert!(!lowered.units.is_empty(), "{name}");
    }
}

#[test]
fn reachable_unreachable_placeholder_is_rejected() {
    use hirsel_lir::{ir::Terminator, validate::ViolationKind};

    let (program, mut lowered) = lower_valid(WITNESS);
    let unit = lowered
        .units
        .iter_mut()
        .find(|u| u.header.definition.0.name == "value")
        .unwrap();
    let terminal = unit
        .terminators
        .iter_mut()
        .find(|t| matches!(t, Terminator::Return(_)))
        .unwrap();
    *terminal = Terminator::Unreachable;
    assert!(violations(&program, &lowered).iter().any(|(_, errors)| {
        errors
            .iter()
            .any(|e| e.kind == ViolationKind::UnprovedUnreachable)
    }));
}

#[test]
fn proven_match_native_parity() {
    use hirsel_driver::{compiler_adapters, native::*};
    use hirsel_engine::*;

    with_compiler_stack(|| {
        let matrix = [
            Engine::Heap { campaign: false },
            Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
            Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        ]
        .map(|engine| Configuration {
            options: Options::new(target()),
            engine,
        });
        let directory = tempfile::tempdir().unwrap();
        let toolchain = Toolchain::for_matrix(&directory.path().join("runtime"), &matrix).unwrap();
        for (name, source) in cases() {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine
                .sources
                .add("native-import-state.hir", source)
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
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let path = directory.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            for (configuration, outcome) in
                differential(&validated, &entry, &reference, &matrix, &toolchain, &path)
            {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name}: {configuration}: {outcome:?}"
                );
            }
        }
    });
}
