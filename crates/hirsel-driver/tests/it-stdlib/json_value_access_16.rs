//! Exact-variant vectors adapted from serde_json 20572a6d, with source/V controls.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/json-value-access-16/{name}.hir"))
}

#[test]
fn vectors_interpreter() {
    with_compiler_stack(|| {
        let outcome = run_path_with_step_budget(
            &fixture("vectors"),
            SimulationConfig::default(),
            Some(1_000_000),
            Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{outcome:?}"
        );
    });
}

#[test]
fn vectors_heap_llvm_cranelift() {
    with_compiler_stack(|| {
        let mut loaded = load_program(&fixture("vectors")).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e:?}"));
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
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
        let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("json-value-access-16");
        let toolchain =
            native::Toolchain::for_matrix(&scratch.join("runtime"), &configurations).unwrap();
        let reference = native::Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output: vec![],
            failures: vec![],
            roots: vec![],
        };
        let outcomes = native::differential(
            &validated,
            &loaded.entry,
            &reference,
            &configurations,
            &toolchain,
            &scratch,
        );
        assert_eq!(outcomes.len(), 3);
        for (configuration, outcome) in outcomes {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
        }
    });
}

fn refused(source: &str, rule: &str) {
    let mut loaded = load_program_source(&fixture("vectors"), Some(source)).unwrap();
    let error = loaded
        .check_for_execution()
        .expect_err("source control must reject");
    let ToolError::Query {
        error: QueryError::Diagnostics(diagnostics),
        ..
    } = error
    else {
        panic!("expected diagnostics: {error:?}")
    };
    assert!(
        diagnostics.iter().any(|d| d.rule.as_str() == rule),
        "{diagnostics:?}"
    );
}

const RELATED: &[(&str, &str)] = &[
    ("as_text(value)", "String"),
    ("number_text(value)", "String"),
    ("as_array(value)", "List[std@1/json.Value]"),
    ("as_object(value)", "Map[String, std@1/json.Value]"),
    ("member(value, \"key\")", "std@1/json.Value"),
    ("element(value, 0)", "std@1/json.Value"),
];

#[test]
fn reference_results_cannot_escape_source() {
    with_compiler_stack(|| {
        for (call, payload) in RELATED {
            refused(
                &format!(
                    r#"
fn escape(value: std@1/json.Value) -> Option[{payload}] uses {{}} {{ std@1/json_access.{call} }}
pub fn main() -> () {{ let value: std@1/json.Value = Null(); let _ = escape(value) }}
"#
                ),
                "MODE-1",
            );
        }
    });
}

#[test]
fn live_results_freeze_source() {
    with_compiler_stack(|| {
        for (call, _) in RELATED {
            refused(
                &format!(
                    r#"
pub fn main() -> () {{
  let mut value: std@1/json.Value = Text("original")
  let selected = std@1/json_access.{call}
  value = Null()
  let _ = std@1/option.is_some(selected)
}}
"#
                ),
                "MODE-1",
            );
        }
    });
}

#[test]
fn local_owned_temporary_cannot_be_related_result() {
    with_compiler_stack(|| {
        refused(
            r#"
fn bad(value: std@1/json.Value) -> Option[String] in value {
  // Unlike a fresh global construction, this counted copy retains a local loan.
  let temporary = copy value
  std@1/json_access.as_text(temporary)
}
pub fn main() -> () { let value: std@1/json.Value = Null(); let _ = bad(value) }
"#,
            "MODE-1",
        );
    });
}

#[test]
fn scalar_result_has_no_source_lifetime() {
    with_compiler_stack(|| {
        let source = r#"
fn flag() -> Option[bool] {
  let value: std@1/json.Value = Bool(false)
  std@1/json_access.as_bool(value)
}
pub fn main() -> Result[(), ()] {
  failure.check(flag() == Some(false), TextInvariant())
  Ok(())
}
"#;
        let mut loaded = load_program_source(&fixture("vectors"), Some(source)).unwrap();
        loaded.check_for_execution().unwrap();
        let observed = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry,
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                observed.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            observed.outcome
        );
    });
}

#[test]
fn independent_v_rejects_wrong_source_and_expired_loan() {
    use hirsel_tir::{Instruction, InstructionKind, Terminator};
    with_compiler_stack(|| {
        let mut loaded = load_program(&fixture("validation")).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        loaded.engine.import_native_input(input.clone()).unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let definition = input
            .definitions
            .values()
            .find(|d| d.path.path().0.name == "select")
            .unwrap();
        hirsel_validate::assert_valid(definition, &input.types, &signatures);
        let mut wrong = (**definition).clone();
        // Same types, different lifetime: the declared result may only use value.
        wrong.bodies[0].parameters.swap(0, 1);
        let report = hirsel_validate::validate(&wrong, &input.types, &signatures, &mut vec![]);
        assert!(
            report.violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::InvalidResultSource(_)
            )),
            "{report:?}"
        );
        let mut expired = (**definition).clone();
        let body = &mut expired.bodies[0];
        let loan = body
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .find_map(|i| match i.kind {
                InstructionKind::BeginLoan { id, .. } => Some(id),
                _ => None,
            })
            .expect("selector must borrow source");
        let block = body
            .blocks
            .iter_mut()
            .find(|b| matches!(b.terminator, Terminator::Call { .. }))
            .unwrap();
        block.instructions.push(Instruction {
            kind: InstructionKind::EndLoan {
                loan,
                if_active: false,
            },
            span: block.span,
            on_failure: hirsel_tir::CleanupId(0),
        });
        let report = hirsel_validate::validate(&expired, &input.types, &signatures, &mut vec![]);
        assert!(
            report
                .violations
                .iter()
                .any(|v| matches!(v.kind, hirsel_validate::ViolationKind::ExpiredLoan { .. })),
            "{report:?}"
        );
    });
}

#[test]
fn httpcheck_field_text_before_and_after_policy() {
    with_compiler_stack(|| {
        let caller = include_str!("../../../../examples/httpcheck/main.hir");
        let current = caller
            .split("fn field_text(")
            .nth(1)
            .unwrap()
            .split("\nfn headers(")
            .next()
            .unwrap();
        let current = format!("fn field_text({current}");
        // Original field_text at launch base 33b9ee32; both execute the same
        // independent expectations, including the existing owned result policy.
        let original = r#"
fn field_text(fields: Map[String, json.Value], key: String) -> Result[unique String, AppError] uses {} {
  let Some(Text(value)) = map.get(fields, key) else { return Error(Scenario()) }
  Ok(unique value)
}
"#;
        let controls = r#"
pub type AppError = Scenario | Transport | Assertion | Output | Authority
fn owned() -> unique String {
  let mut fields: Map[String, json.Value] = map.empty()
  map.insert(mut fields, "name", Text("snow ☃"))
  let Ok(text) = field_text(fields, "name") else { failure.check(false, TextInvariant()); return "" }
  take text
}
pub fn main() -> Result[(), ()] {
  let empty: Map[String, json.Value] = map.empty()
  failure.check(field_text(empty, "missing") == Error(Scenario()), TextInvariant())
  let wrong: List[json.Value] = [Null(), Bool(false), Number("1e+02"), Array([]), Object(map.empty())]
  for value in wrong {
    let mut fields: Map[String, json.Value] = map.empty()
    map.insert(mut fields, "key", copy value)
    failure.check(field_text(fields, "key") == Error(Scenario()), TextInvariant())
  }
  let mut fields: Map[String, json.Value] = map.empty()
  map.insert(mut fields, "", Text(""))
  failure.check(field_text(fields, "") == Ok(""), TextInvariant())
  failure.check(owned() == "snow ☃", TextInvariant())
  Ok(())
}
"#;
        for (label, helper) in [("before", original), ("after", current.as_str())] {
            let source = format!("{helper}\n{controls}");
            let mut loaded = load_program_source(&fixture("vectors"), Some(&source)).unwrap();
            loaded
                .check_for_execution()
                .unwrap_or_else(|e| panic!("{label}: {e:?}"));
            let observed = run_observed_snapshot(
                &mut loaded.engine,
                &RunConfiguration {
                    entry: loaded.entry,
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            assert!(
                matches!(
                    observed.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "{label}: {:?}",
                observed.outcome
            );
            eprintln!(
                "httpcheck field_text {label}: 1 executed program, 8 independent policy assertions passed"
            );
        }
    });
}
