//! RFC6901 reference/independent vectors and borrowing controls.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/json-pointer-8-vectors.hir")
}

#[test]
fn vectors_on_h_llvm_and_cranelift() {
    with_compiler_stack(|| {
        let mut loaded = load_program(&fixture()).unwrap();
        loaded.check_for_execution().unwrap_or_else(|e| match e {
            ToolError::Query { error, .. } => panic!("{error:?}"),
            e => panic!("{e}"),
        });
        let reference = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                reference.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            reference.outcome
        );
        let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let configs = vec![
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
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("json-pointer-8-vectors");
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
        for (config, outcome) in
            native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir)
        {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{config}: {outcome:?}"
            );
        }
        eprintln!("json-pointer-8: H, LLVM and Cranelift each executed the fixed vectors");
    });
}

fn refused(source: &str, rule: &str) {
    with_compiler_stack(|| {
        let mut loaded = load_program_source(&fixture(), Some(source)).unwrap();
        let error = loaded
            .check_for_execution()
            .expect_err("ownership control must reject");
        let ToolError::Query {
            error: QueryError::Diagnostics(diagnostics),
            ..
        } = error
        else {
            panic!("expected ownership diagnostics: {error:?}")
        };
        assert_eq!(
            diagnostics.first().map(|d| d.rule.as_str()),
            Some(rule),
            "{diagnostics:?}"
        );
        eprintln!("json-pointer-8 refused ownership control: {diagnostics:?}");
    });
}

#[test]
fn result_cannot_outlive_document() {
    refused(
        r#"
fn escape(document: std@1/json.Value, pointer: std@1/json_pointer.Pointer) -> Option[std@1/json.Value] uses {} {
  std@1/json_pointer.get(document, pointer)
}
pub fn main() -> () {
  let Ok(pointer) = std@1/json_pointer.parse("") else { return }
  let document: std@1/json.Value = Text("owned")
  let _ = escape(document, pointer)
}
"#,
        "MODE-1",
    );
}

#[test]
fn result_cannot_mutate_document() {
    // LANG-5 access checking rejects immutable `items` first. This alone does
    // not prove referent safety; the live-selection control below checks that.
    refused(
        r#"
pub fn main() -> () {
  let document: std@1/json.Value = Array([Text("original")])
  let Ok(pointer) = std@1/json_pointer.parse("") else { return }
  let Some(selected) = std@1/json_pointer.get(document, pointer) else { return }
  match selected {
    Array(items) => core@1/list.push(mut items, Text("mutated")),
    _ => (),
  }
}
"#,
        "LANG-5",
    );
}

#[test]
fn owned_copy_mutation_leaves_document_unchanged() {
    with_compiler_stack(|| {
        let mut loaded = load_program_source(
            &fixture(),
            Some(
                r#"
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let document: std@1/json.Value = Array([Text("original")])
  let Ok(pointer) = std@1/json_pointer.parse("") else {
    core@1/failure.check(false, TextInvariant())
    return Ok(())
  }
  let Some(selected) = std@1/json_pointer.get(document, pointer) else {
    core@1/failure.check(false, TextInvariant())
    return Ok(())
  }
  let mut Array(owned) = copy selected else {
    core@1/failure.check(false, TextInvariant())
    return Ok(())
  }
  core@1/list.push(mut owned, Text("mutated"))
  core@1/failure.check(core@1/list.len(owned) == 2u64, TextInvariant())
  match owned[1u64] {
    Text(s) => core@1/failure.check(s == "mutated", TextInvariant()),
    _ => core@1/failure.check(false, TextInvariant()),
  }
  match std@1/json_pointer.get(document, pointer) {
    Some(Array(original)) => {
      core@1/failure.check(core@1/list.len(original) == 1u64, TextInvariant())
      match original[0u64] {
        Text(s) => core@1/failure.check(s == "original", TextInvariant()),
        _ => core@1/failure.check(false, TextInvariant()),
      }
    },
    _ => core@1/failure.check(false, TextInvariant()),
  }
  Ok(())
}
"#,
            ),
        )
        .unwrap();
        loaded.check_for_execution().unwrap();
        let result = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                result.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            result.outcome
        );
    });
}

#[test]
fn selection_freezes_document_until_last_use() {
    refused(
        r#"
pub fn main() -> () {
  let mut document: std@1/json.Value = Text("original")
  let Ok(pointer) = std@1/json_pointer.parse("") else { return }
  let selected = std@1/json_pointer.get(document, pointer)
  document = Null()
  match selected { Some(Text(s)) => core@1/failure.check(s == "original", TextInvariant()), _ => () }
}
"#,
        "MODE-1",
    );
}
