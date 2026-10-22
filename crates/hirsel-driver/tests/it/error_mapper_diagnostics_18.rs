//! LANG-8 explicit mapper explanations and checked named payload-discard repairs.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn checked(source: &str) -> CheckOutput {
    let mut engine = crate::source_std::engine();
    let file = engine.sources.add("mapper.hir", source.into()).unwrap();
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

#[test]
fn error_mapper_diagnostics_18_rejected_signatures_and_utf8_labels() {
    with_compiler_stack(|| {
        for (arm, mapper, declaration) in [
            ("InvalidJson()", "InvalidJson", ""),
            ("InvalidJson(u64)", "InvalidJson", ""),
            ("InvalidJson(ParseError, u64)", "InvalidJson", ""),
            (
                "InvalidJson()",
                "convert",
                "fn convert() -> AppError { InvalidJson() }",
            ),
            (
                "InvalidJson()",
                "convert",
                "fn convert(e: ParseError, n: u64) -> AppError { InvalidJson() }",
            ),
            (
                "InvalidJson()",
                "convert",
                "fn convert(e: u64) -> AppError { InvalidJson() }",
            ),
            (
                "InvalidJson()",
                "convert",
                "fn convert(e: ParseError) -> u64 { 0 }",
            ),
            (
                "InvalidJson()",
                "convert",
                "fn convert(mut e: ParseError) -> AppError { InvalidJson() }",
            ),
        ] {
            let source = format!(
                "// café 🐑\npub type ParseError = Empty()\npub type AppError = {arm}\n{declaration}\nfn parse() -> Result[u64, ParseError] {{ Error(Empty()) }}\nfn run() -> Result[u64, AppError] {{ let n = parse() ? {mapper}\nOk(n) }}\n"
            );
            let output = checked(&source);
            let errors: Vec<_> = output
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            let [d] = &errors[..] else {
                panic!("{source}: {errors:?}")
            };
            assert_eq!(d.rule.as_str(), "LANG-8", "{d:?}");
            let primary = format!("parse() ? {mapper}");
            assert_eq!(output.sources.slice(d.primary), Some(primary.as_str()));
            assert_eq!(
                output.sources.resolve(d.primary).start as usize,
                source.find(&primary).unwrap()
            );
            for (site, message) in [
                ("parse()", "propagates `ParseError`"),
                (
                    "Result[u64, AppError]",
                    "enclosing error channel is `AppError`",
                ),
            ] {
                assert!(d.secondary.iter().any(|l| output.sources.slice(l.span) == Some(site) && l.message == message), "{d:?}");
            }
            let name = if declaration.is_empty() {
                "AppError"
            } else {
                "convert"
            };
            assert!(
                d.secondary
                    .iter()
                    .any(|l| output.sources.slice(l.span) == Some(name)
                        && l.message.contains("mapper")),
                "{d:?}"
            );
            let human = d.render_human(&output.sources);
            assert!(human.contains("fn(ParseError) -> AppError"), "{human}");
            if declaration.contains("mut e") {
                assert!(human.contains("fn(mut ParseError) -> AppError"), "{human}");
            }
            assert!(d.fix.is_none());
            for label in &d.secondary {
                assert!(human.contains(&label.message), "{human}");
            }
        }
    });
}

const ORIGINAL: &str = "// café 🐑\npub type CommandError = InvalidJson()\nfn validate(candidate: json.Value) -> Result[(), CommandError] uses {} {\n let encoded = json.stringify(candidate) ? InvalidJson\n let _ = encoded\n Ok(())\n}\n";

#[test]
fn error_mapper_diagnostics_18_dogfood_json_helper_checks_and_executes() {
    with_compiler_stack(|| {
        let output = checked(ORIGINAL);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "LANG-8", "{d:?}");
        assert_eq!(
            output.sources.slice(d.primary),
            Some("json.stringify(candidate) ? InvalidJson")
        );
        assert!(
            d.notes
                .iter()
                .any(|n| n.contains("fn(std@1/json/JsonError) -> CommandError")),
            "{d:?}"
        );
        let help = d
            .notes
            .iter()
            .find(|n| n.starts_with("help: to intentionally discard"))
            .unwrap();
        let helper = help
            .split('`')
            .find(|s| s.starts_with("fn map_error("))
            .unwrap();
        let repaired = ORIGINAL.replace("? InvalidJson", "? map_error")
            + "\n"
            + helper
            + r#"

pub type ParseError = Empty()
pub type Envelope[E] = Wrapped(E)
fn parse(ok: bool) -> Result[u64, ParseError] { if ok { Ok(7) } else { Error(Empty()) } }
fn wrap[E](take error: E) -> Envelope[E] uses {} { Wrapped(take error) }
fn take_wrap(take error: ParseError) -> Envelope[ParseError] uses {} { Wrapped(take error) }
fn mapped(ok: bool) -> Result[u64, Envelope[ParseError]] { let n = parse(ok) ? wrap; Ok(n) }
fn taken() -> Result[u64, Envelope[ParseError]] { let n = parse(false) ? take_wrap; Ok(n) }
fn variant() -> Result[u64, Envelope[ParseError]] { let n = parse(false) ? Wrapped; Ok(n) }
pub fn main() -> Result[(), String] {
  if validate(Null()) != Ok(()) { return Error("success path") }
  if validate(Number("invalid")) != Error(InvalidJson()) { return Error("discarded payload") }
  if mapped(true) != Ok(7) { return Error("generic success") }
  if mapped(false) != Error(Wrapped(Empty())) { return Error("generic error") }
  if taken() != Error(Wrapped(Empty())) { return Error("taken error") }
  if variant() != Error(Wrapped(Empty())) { return Error("variant error") }
  Ok(())
}
"#;
        let mut engine = crate::source_std::engine();
        let file = engine.sources.add("repaired.hir", repaired.into()).unwrap();
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
        assert!(reference.failures.is_empty());
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let configurations = vec![native::Configuration {
            options: native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            engine: native::Engine::Native {
                opt: native::OptLevel::Speed,
                partition: native::Partition::Whole,
                runtime: native::Runtime::Diagnostic,
            },
        }];
        let scratch =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("error-mapper-diagnostics-18");
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
        assert_eq!(outcomes.len(), 1);
        for (configuration, outcome) in outcomes {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
        }
    });
}

#[test]
fn error_mapper_diagnostics_18_closure_uses_its_own_error_channel() {
    with_compiler_stack(|| {
        let output = checked(
            "pub type E = Bad()\npub type Outer = Other()\nfn source() -> Result[u64, String] { Error(\"x\") }\nfn outer() -> Result[(), Outer] { let f = fn() -> Result[u64, E] { let n = source() ? Bad; Ok(n) }; let _ = f; Ok(()) }",
        );
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "LANG-8", "{d:?}");
        assert!(
            d.secondary
                .iter()
                .any(|l| output.sources.slice(l.span) == Some("Result[u64, E]")
                    && l.message == "enclosing error channel is `E`")
        );
        assert!(d.notes.iter().any(|n| n.contains("fn(String) -> E")));
    });
}
