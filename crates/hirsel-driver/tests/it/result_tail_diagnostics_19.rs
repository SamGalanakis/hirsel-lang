//! Independent expected diagnostics and executable repairs for Result-valued tails.
use hirsel_driver::*;
use hirsel_engine::*;

const UNIT: &str = "fn unit() -> Result[(), ()] {\n  Ok(())\n}\n\npub fn main() -> Result[(), ()] {\n  unit()?\n}\n";
const TYPES: &str = "type E = Bad(u64)\ntype App = Input(E)\nfn operation(ok: bool) -> Result[u64, E] { if ok { Ok(7) } else { Error(Bad(19)) } }\n";

fn checked(source: &str) -> CheckOutput {
    let mut engine = crate::source_std::engine();
    let file = engine
        .sources
        .add("result-tail.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    let warm = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(
        output
            .diagnostics
            .iter()
            .map(|d| d.to_json(&output.sources))
            .collect::<Vec<_>>(),
        warm.diagnostics
            .iter()
            .map(|d| d.to_json(&warm.sources))
            .collect::<Vec<_>>()
    );
    output
}

fn rejected(source: &str, primary: &str, payload: &str, direct: bool) {
    let output = checked(source);
    let [d] = &output.diagnostics[..] else {
        panic!("{source}: {:?}", output.diagnostics)
    };
    assert_eq!(d.rule.as_str(), "LANG-7", "{d:?}");
    assert_eq!(output.sources.slice(d.primary), Some(primary), "{d:?}");
    assert!(d.fix.is_none());
    let human = d.render_human(&output.sources);
    let json = d.to_json(&output.sources);
    eprintln!("{source}\n{human}\n{json}");
    assert!(
        human.contains(&format!("success payload `{payload}`")),
        "{human}"
    );
    let notes = json["notes"].as_array().unwrap();
    assert!(
        notes
            .iter()
            .any(|n| n.as_str().unwrap().contains("success payload")),
        "{json}"
    );
    for note in notes {
        assert!(human.contains(note.as_str().unwrap()), "{human}");
    }
    assert_eq!(human.contains("omit `?`"), direct, "{human}");
    assert!(
        human.contains(if source.contains("? Input") {
            "Ok(...)"
        } else if payload == "()" {
            "operation()?; Ok(())"
        } else {
            "Ok(operation()?)"
        }),
        "{human}"
    );
}

#[test]
fn result_tail_diagnostics_19_retained_unit() {
    with_compiler_stack(|| rejected(UNIT, "unit()", "()", true));
}

#[test]
fn result_tail_diagnostics_19_nonunit_block_if_return_closure() {
    with_compiler_stack(|| {
        for body in [
            "fn run() -> Result[u64, E] { operation(true)? }",
            "fn run() -> Result[u64, E] { { operation(true)? } }",
            "fn run(ok: bool) -> Result[u64, E] { if ok { operation(true)? } else { Ok(2) } }",
            "fn run(ok: bool) -> Result[u64, E] { if ok { return operation(true)? }; Ok(2) }",
            "fn outer() { let f = fn() -> Result[u64, E] { operation(true)? }; let _ = f }",
        ] {
            rejected(
                &format!("// café 🐑\n{TYPES}{body}\n"),
                "operation(true)",
                "u64",
                true,
            );
            let repair = body.replace("operation(true)?", "Ok(operation(true)?)");
            let output = checked(&format!("{TYPES}{repair}\n"));
            assert!(
                output.diagnostics.is_empty(),
                "{repair}: {:?}",
                output.diagnostics
            );
        }
    });
}

#[test]
fn result_tail_diagnostics_19_injection_and_explicit_mapper() {
    with_compiler_stack(|| {
        for suffix in ["?", "? Input"] {
            let source =
                format!("{TYPES}fn run() -> Result[u64, App] {{ operation(true){suffix} }}\n");
            rejected(&source, "operation(true)", "u64", false);
            let repaired = source.replace(
                &format!("operation(true){suffix}"),
                &format!("Ok(operation(true){suffix})"),
            );
            assert!(checked(&repaired).diagnostics.is_empty(), "{repaired}");
        }
    });
}

#[test]
fn result_tail_diagnostics_19_resource_payload() {
    with_compiler_stack(|| {
        let source = "resource type R: noalloc_drop { items: List[String] }\n@noalloc drop(self) {}\nfn operation() -> Result[R, ()] { Ok(R(items: [\"kept\"])) }\nfn run() -> Result[R, ()] { operation()? }\n";
        rejected(source, "operation()", "R", true);
        assert!(
            checked(&source.replace("{ operation()? }", "{ Ok(operation()?) }"))
                .diagnostics
                .is_empty()
        );
    });
}

#[test]
fn result_tail_diagnostics_19_earlier_failures_and_wrong_payloads() {
    with_compiler_stack(|| {
        for (source, rule) in [
            (
                format!(
                    "{TYPES}fn run() -> Result[u64, App] {{ let n = operation(true) ? App; Ok(n) }}\n"
                ),
                "LANG-8",
            ),
            (
                format!("{TYPES}fn run() -> Result[String, E] {{ operation(true)? }}\n"),
                "LANG-7",
            ),
            (
                "fn operation() -> u64 { 7 }\nfn run() -> Result[u64, ()] { operation()? }\n"
                    .into(),
                "LANG-8",
            ),
            (
                "fn run() -> Result[u64, ()] { missing()? }\n".into(),
                "LANG-4",
            ),
        ] {
            let output = checked(&source);
            assert_eq!(
                output.diagnostics.len(),
                1,
                "{source}: {:?}",
                output.diagnostics
            );
            let d = &output.diagnostics[0];
            assert_eq!(d.rule.as_str(), rule, "{source}: {d:?}");
            assert!(
                !d.notes.iter().any(|n| n.contains("Ok(operation()?)")),
                "{d:?}"
            );
            eprintln!("control: {source}\n{}", d.render_human(&output.sources));
        }
    });
}

#[test]
fn result_tail_diagnostics_19_checked_unit_and_direct_return() {
    with_compiler_stack(|| {
        for source in [
            UNIT.replace("unit()?", "unit()?; Ok(())"),
            UNIT.replace("unit()?", "Ok(unit()?)"),
            UNIT.replace("unit()?", "unit()"),
            format!("{TYPES}fn run() -> Result[u64, E] {{ operation(true) }}\n"),
        ] {
            let output = checked(&source);
            assert!(
                output.diagnostics.is_empty(),
                "{source}: {:?}",
                output.diagnostics
            );
        }
    });
}

#[test]
fn result_tail_diagnostics_19_invalid_tail_mapping_has_no_repair() {
    with_compiler_stack(|| {
        for suffix in ["? App", "?"] {
            let source = format!(
                "type E = Bad(u64)\ntype App = Other()\nfn operation() -> Result[u64, E] {{ Error(Bad(19)) }}\nfn run() -> Result[u64, App] {{ operation(){suffix} }}\n"
            );
            let output = checked(&source);
            assert_eq!(output.diagnostics[0].rule.as_str(), "LANG-7");
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.rule.as_str() == "LANG-8"),
                "{:?}",
                output.diagnostics
            );
            assert!(
                !output
                    .diagnostics
                    .iter()
                    .flat_map(|d| &d.notes)
                    .any(|n| n.contains("Ok(operation()?)") || n.contains("omit `?`"))
            );
        }
    });
}

const REPAIRED: &str = r#"
type E = Bad(u64)
type App = Input(E)
fn operation(ok: bool) -> Result[u64, E] { if ok { Ok(7) } else { Error(Bad(19)) } }
fn unit(ok: bool) -> Result[(), E] { if ok { Ok(()) } else { Error(Bad(19)) } }
fn unit_repair(ok: bool) -> Result[(), E] { unit(ok)?; Ok(()) }
fn payload_repair(ok: bool) -> Result[u64, E] { Ok(operation(ok)?) }
fn inject_repair(ok: bool) -> Result[u64, App] { Ok(operation(ok)?) }
fn mapper_repair(ok: bool) -> Result[u64, App] { Ok(operation(ok) ? Input) }
fn direct(ok: bool) -> Result[u64, E] { operation(ok) }
fn block_repair(ok: bool) -> Result[u64, E] { { Ok(operation(ok)?) } }
fn if_repair(ok: bool) -> Result[u64, E] { if ok { Ok(operation(ok)?) } else { Ok(operation(ok)?) } }
fn return_repair(ok: bool) -> Result[u64, E] { if ok { return Ok(operation(ok)?) }; Ok(operation(ok)?) }
fn counted(mut count: u64, ok: bool) -> Result[u64, E] { count = count + 1; operation(ok) }
fn counted_repair(mut count: u64, ok: bool) -> Result[u64, App] { Ok(counted(mut count, ok)?) }
resource type R: noalloc_drop { n: u64 }
@noalloc drop(self) {}
fn resource_operation(ok: bool) -> Result[R, E] { if ok { Ok(R(n: 7)) } else { Error(Bad(19)) } }
fn resource_repair(ok: bool) -> Result[R, App] { Ok(resource_operation(ok)?) }
pub fn main() -> Result[(), String] {
  for ok in [true, false] {
    let wanted: Result[u64, E] = if ok { Ok(7) } else { Error(Bad(19)) }
    let injected: Result[u64, App] = if ok { Ok(7) } else { Error(Input(Bad(19))) }
    let wanted_unit: Result[(), E] = if ok { Ok(()) } else { Error(Bad(19)) }
    if unit_repair(ok) != wanted_unit { return Error("unit") }
    if payload_repair(ok) != wanted { return Error("payload") }
    if direct(ok) != wanted { return Error("direct") }
    if block_repair(ok) != wanted { return Error("block") }
    if if_repair(ok) != wanted { return Error("if") }
    if return_repair(ok) != wanted { return Error("return") }
    if inject_repair(ok) != injected { return Error("inject") }
    if mapper_repair(ok) != injected { return Error("mapper") }
    let f = fn() -> Result[u64, E] { Ok(operation(ok)?) }
    if f() != wanted { return Error("closure") }
    let mut count = 0u64
    if counted_repair(mut count, ok) != injected { return Error("counted") }
    if count != 1 { return Error("duplicated execution") }
    match resource_repair(ok) {
      Ok(r) => { if !ok || r.n != 7 { return Error("resource success") } },
      Error(e) => { if ok || e != Input(Bad(19)) { return Error("resource error") } },
    }
  }
  Ok(())
}
"#;

#[test]
fn result_tail_diagnostics_19_repairs_h_llvm_cranelift() {
    with_compiler_stack(|| {
        let mut engine = crate::source_std::engine();
        let file = engine.sources.add("repairs.hir", REPAIRED.into()).unwrap();
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
        let reference = native::Observation::of_reference(execution.observation.as_ref().unwrap());
        assert_eq!(reference.exit_code, Some(0), "{reference:?}");
        assert!(reference.failures.is_empty(), "{reference:?}");
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
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
        let scratch = tempfile::Builder::new()
            .prefix("result-tail-diagnostics-19-")
            .tempdir()
            .unwrap();
        let toolchain =
            native::Toolchain::for_matrix(&scratch.path().join("runtime"), &configurations)
                .unwrap();
        let outcomes = native::differential(
            &validated,
            &entry,
            &reference,
            &configurations,
            &toolchain,
            scratch.path(),
        );
        assert_eq!(outcomes.len(), 3);
        for (configuration, outcome) in outcomes {
            eprintln!("result-tail-diagnostics-19: {configuration}: {outcome:?}");
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
        }
    });
}
