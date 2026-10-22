//! LANG-13 rejects Rust-style nested binding mutability. Recipes below retain
//! Hirsel's existing physical/taken pattern rules, without match ergonomics.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

#[test]
fn mutable_pattern_diagnostics_17_rejected_originals() {
    with_compiler_stack(|| {
        for source in [
            "fn read(d: Dir) -> Result[u64, FileError] { let Ok(mut f) = file.open(d, path.of(\"input\")) else { return Error(NotFound()) }; let mut data: List[u8] = []; file.read(d, mut f, mut data, 3) }",
            "fn example(value: Option[u64]) -> u64 { let Some(mut n) = value else { return 0 }; n += 1; n }",
            "fn f(value: Option[Bytes]) { if let Some(mut data) = value {} }",
            "fn f(value: Option[Bytes]) { match value { Some(mut data) => (), None() => () } }",
            "fn f(value: Option[Option[Bytes]]) { let Some(Some(mut data)) = value else { return } }",
            "fn f(value: Option[Bytes], pattern_value: u64) { let Some(mut data) = value else { return }; let mutable_value = pattern_value; let pattern_value_2 = mutable_value }",
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("original.hir", source.into()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(
                output.diagnostics.len(),
                1,
                "{source}: {:?}",
                output.diagnostics
            );
            let d = &output.diagnostics[0];
            assert_eq!(d.rule.as_str(), "LANG-13");
            assert_eq!(output.sources.slice(d.primary), Some("mut"));
            assert!(d.fix.is_none());
            assert!(
                d.notes.iter().any(|note| note.contains("read-only")),
                "{d:?}"
            );
            assert!(
                d.notes
                    .iter()
                    .any(|note| note.contains("take pattern_value")),
                "{d:?}"
            );
            let json = d.to_json(&output.sources);
            let text = d.render_human(&output.sources);
            for note in json["notes"].as_array().unwrap() {
                assert!(
                    text.contains(
                        note.as_str()
                            .unwrap()
                            .strip_prefix("help: ")
                            .unwrap_or(note.as_str().unwrap())
                    ),
                    "{text}"
                );
            }
        }
    });
}

// Independently written semantic oracles: one position increment, 3-byte
// borrowed payload, untouched 41/43 collision sentinels, and diverging failure.
const REPAIRED: &str = r#"
resource type FileLike { position: u64 }
drop(f) {}
fn advance(mut f: FileLike) { f.position += 1 }
fn owned(take value: Result[FileLike, ()]) -> u64 {
  let Ok(pattern_value) = take value else { return 90 }
  let mut mutable_value = take pattern_value
  advance(mut mutable_value)
  mutable_value.position
}
fn counted(value: Option[Bytes]) -> u64 {
  if let Some(pattern_value) = value {
    let mut mutable_value = pattern_value
    mutable_value = pattern_value
    bytes.len(mutable_value)
  } else { 0 }
}
fn nested(value: Option[Option[Bytes]]) -> u64 {
  match value {
    Some(Some(pattern_value)) if bytes.len(pattern_value) == 3 => {
      let mut mutable_value = pattern_value
      mutable_value = pattern_value
      bytes.len(mutable_value)
    }
    _ => 0
  }
}
fn collision(value: Option[u64], pattern_value: u64) -> u64 {
  let mutable_value = 43u64
  let Some(pattern_value_2) = value else { return 0 }
  let mut mutable_value_2 = pattern_value_2
  mutable_value_2 += 1
  mutable_value_2 + pattern_value + mutable_value
}
pub fn main() -> Result[(), ()] {
  let a: Result[FileLike, ()] = Ok(FileLike(position: 6))
  failure.check(owned(take a) == 7, Arithmetic())
  let b: Result[FileLike, ()] = Error(())
  failure.check(owned(take b) == 90, Arithmetic())
  let c: Option[Bytes] = Some(b"abc")
  failure.check(counted(c) == 3, Arithmetic())
  failure.check(counted(c) == 3, Arithmetic())
  failure.check(counted(None()) == 0, Arithmetic())
  failure.check(nested(Some(Some(b"abc"))) == 3, Arithmetic())
  failure.check(nested(Some(Some(b"ab"))) == 0, Arithmetic())
  failure.check(nested(None()) == 0, Arithmetic())
  failure.check(collision(Some(2), 41) == 87, Arithmetic())
  Ok(())
}
"#;

#[test]
fn mutable_pattern_diagnostics_17_checked_repairs_execute_on_both_backends() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("repaired.hir", REPAIRED.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let entry = entry_path(&default_context());
        let execution = run_observed_checked(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let observation = execution.observation.as_ref().unwrap();
        assert_eq!(
            observation
                .effects
                .iter()
                .filter(|effect| matches!(
                    effect.effect,
                    hirsel_interp::observation::Effect::Finalizer { .. }
                ))
                .count(),
            1,
            "the transferred FileLike must be finalized exactly once"
        );
        let reference = native::Observation::of_reference(observation);
        assert_eq!(reference.exit_code, Some(0), "{reference:?}");
        assert!(reference.failures.is_empty());
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let configurations = vec![
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
        let scratch =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mutable-pattern-diagnostics-17");
        std::fs::create_dir_all(&scratch).unwrap();
        let toolchain =
            native::Toolchain::for_matrix(&scratch.join("runtime"), &configurations).unwrap();
        let outcomes = native::differential(
            &validated,
            &entry,
            &reference,
            &configurations,
            &toolchain,
            &scratch,
        );
        assert_eq!(outcomes.len(), 2);
        for (configuration, outcome) in outcomes {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
        }
    });
}

#[test]
fn mutable_pattern_diagnostics_17_borrowed_payload_cannot_be_taken_or_mutated() {
    with_compiler_stack(|| {
        for (action, rule) in [
            (
                "let mut cursor = take payload; let _ = list.len(cursor)",
                "OWN-1",
            ),
            ("let mut cursor = payload; cursor[0u64] = 1u8", "LANG-15"),
        ] {
            let source = format!(
                "fn probe(value: Option[List[u8]]) {{ if let Some(payload) = value {{ {action} }} }}"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("borrowed.hir", source).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(output.diagnostics.len(), 1, "{:?}", output.diagnostics);
            assert_eq!(
                output.diagnostics[0].rule.as_str(),
                rule,
                "{:?}",
                output.diagnostics
            );
        }
    });
}
