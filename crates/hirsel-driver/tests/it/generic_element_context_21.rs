//! Nested generic element contracts explain suppliers without widening inference.
use hirsel_driver::*;
use hirsel_engine::*;

const HEADER: &str = "fn need[T](take x:unique T) {}\nfn relay[T](take source:unique List[unique T]) {for x in take source {need(take x)}}\n";

fn source(body: &str) -> String {
    format!("{HEADER}pub fn main()->Result[(),()] {{{body};Ok(())}}")
}

fn checked(source: &str) -> CheckOutput {
    let mut engine = crate::source_std::engine();
    checked_in(&mut engine, source)
}

fn checked_in(engine: &mut QueryEngine, source: &str) -> CheckOutput {
    let file = engine
        .sources
        .add("generic-element.hir", source.into())
        .unwrap();
    let output = check_progress(engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    let warm = check_progress(engine, file, &default_context()).unwrap();
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

fn accepted(source: &str) {
    let mut engine = crate::source_std::engine();
    accepted_in(&mut engine, source);
}

fn accepted_in(engine: &mut QueryEngine, source: &str) {
    let file = engine
        .sources
        .add("generic-element.hir", source.into())
        .unwrap();
    let output = check_progress(engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    assert!(
        output.diagnostics.is_empty(),
        "{source}\n{:?}",
        output.diagnostics
    );
    let input = engine.export_native_input().unwrap();
    engine.import_native_input(input).unwrap();
}

#[test]
fn generic_element_context_21_inline_names_nested_contract_and_supplier() {
    with_compiler_stack(|| {
        let output = checked(&source("relay([\"a\"])"));
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        eprintln!(
            "{}\n{}",
            d.render_human(&output.sources),
            d.to_json(&output.sources)
        );
        assert_eq!(d.rule.as_str(), "MODE-2");
        assert_eq!(output.sources.slice(d.primary), Some("[\"a\"]"));
        assert!(
            d.secondary
                .iter()
                .any(|l| output.sources.slice(l.span) == Some("relay")
                    && l.message
                        .contains("nested element contract `unique String`")),
            "{d:?}"
        );
        assert!(
            d.secondary
                .iter()
                .any(|l| output.sources.slice(l.span) == Some("[\"a\"]")
                    && l.message.contains("supplier")),
            "{d:?}"
        );
        assert_eq!(d.secondary.len(), 2, "{d:?}");
        let human = d.render_human(&output.sources);
        let json = d.to_json(&output.sources);
        assert_eq!(json["message"], d.message);
        for label in &d.secondary {
            assert!(human.contains(&label.message));
        }
        for note in json["notes"].as_array().unwrap() {
            assert!(human.contains(note.as_str().unwrap()));
        }
        assert!(!human.contains("End live shares"));
        assert!(human.contains("outer List alone is insufficient"));
    });
}

#[test]
fn generic_element_context_21_inference_and_storage_controls() {
    with_compiler_stack(|| {
        accepted(&source(
            "let source:unique List[unique String]=[\"a\"];relay(take source)",
        ));
        accepted(
            &source("relay([\"a\"])")
                .replace("fn relay[T]", "fn relay")
                .replace("List[unique T]", "List[unique String]"),
        );
        // Expression type arguments are deliberately unsupported (LANG-2).
        let output = checked(&source("relay[String]([\"a\"])"));
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "LANG-2"
                    && d.message.contains("type arguments are never written"))
        );
        // A root annotation leaves the missing nested contract unchanged.
        let output = checked(&source(
            "let source:unique List[String]=[\"a\"];relay(take source)",
        ));
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "MODE-2");
        assert_eq!(output.sources.slice(d.primary), Some("take source"));
        assert!(d.fix.is_none());
        assert!(d.notes.iter().any(|n| n.contains("already shared value")));
    });
}

#[test]
fn generic_element_context_21_machine_repairs_apply_check_and_export() {
    with_compiler_stack(|| {
        // Check every original and repaired program against the same source
        // dependency graph, preparing std once rather than ten times.
        let mut engine = crate::source_std::engine();
        for literal in [
            "[\"a\"]",
            "[\"λ🐑\", \"b\\\"c\"]",
            "[[\"a\"], [\"b\", \"c\"]]",
        ] {
            let text = source(&format!("relay({literal})"));
            let output = checked_in(&mut engine, &text);
            let [d] = &output.diagnostics[..] else {
                panic!("{text}\n{:?}", output.diagnostics)
            };
            let fix = d.fix.as_ref().unwrap_or_else(|| panic!("{d:?}"));
            assert_eq!(output.sources.slice(fix.span), Some(literal));
            assert!(d.notes.iter().any(|n| n.contains(&fix.replacement)));
            let span = output.sources.resolve(fix.span);
            let mut repaired = text.clone();
            repaired.replace_range(span.start as usize..span.end as usize, &fix.replacement);
            accepted_in(&mut engine, &repaired);
        }
        // Literal inference uses core's List even with a user type of that
        // name. The edit must keep referring to the builtin at every depth.
        let text = "type List { n:u64 }\nfn relay[T](take source:unique core@1/List[unique T]){}\npub fn main(){relay([[\"a\"]])}";
        let output = checked_in(&mut engine, text);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        let fix = d.fix.as_ref().unwrap();
        assert!(
            fix.replacement
                .contains("core@1/List[unique core@1/List[String]]"),
            "{fix:?}"
        );
        let span = output.sources.resolve(fix.span);
        let mut repaired = text.to_owned();
        repaired.replace_range(span.start as usize..span.end as usize, &fix.replacement);
        accepted_in(&mut engine, &repaired);
        // A new name cannot capture a local, parameter or module declaration.
        let text = source("let saved_2=0u64;relay([\"a\"]);let _=saved_2");
        let text = format!("fn saved() {{}}\n{text}");
        let output = checked_in(&mut engine, &text);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        let fix = d.fix.as_ref().unwrap();
        assert!(fix.replacement.contains("let saved_3:"), "{fix:?}");
        let span = output.sources.resolve(fix.span);
        let mut repaired = text.clone();
        repaired.replace_range(span.start as usize..span.end as usize, &fix.replacement);
        accepted_in(&mut engine, &repaired);
    });
}

#[test]
fn generic_element_context_21_prefix_and_real_sharing_have_no_annotation_edit() {
    with_compiler_stack(|| {
        for body in ["relay([])", "relay([1,2u64])"] {
            let output = checked(&source(body));
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.rule.as_str() == "LANG-2")
            );
            assert!(
                output
                    .diagnostics
                    .iter()
                    .all(|d| !d.message.contains("nested element contract"))
            );
        }
        let text = source(
            "let x=f\"a\";let alias=copy x;let source:unique List[unique String]=[take x];relay(take source);let _=alias",
        );
        let output = checked(&text);
        assert!(output.diagnostics.iter().any(|d| {
            d.rule.as_str() == "MODE-2"
                && d.secondary
                    .iter()
                    .any(|l| output.sources.slice(l.span) == Some("copy x"))
        }));
        assert!(output.diagnostics.iter().all(|d| {
            d.fix.is_none()
                && !d
                    .notes
                    .iter()
                    .any(|n| n.contains("Give this fresh literal"))
        }));
        accepted(&text.replace("[take x]", "[unique x]"));
        // Named and computed suppliers have no fresh-literal edit, even if a
        // programmer could independently establish a stronger producer.
        for body in [
            "let x=\"a\";relay([take x])",
            "relay([f\"a\"])",
            &format!("relay([\"{}\"])", "x".repeat(257)),
        ] {
            let output = checked(&source(body));
            assert!(!output.diagnostics.is_empty());
            assert!(output.diagnostics.iter().all(|d| d.fix.is_none()));
        }
    });
}

// Rust alloctests/tests/vec.rs:257-292 and alloc/src/vec/mod.rs:4054-4092,
// f6c1107f8be13e48465e12c0fd26e6a39fe18e4f: empty/equal/short/long copies.
// Adapted to observable snapshots: ordinary copy may share, while the generic
// producer explicitly deep-copies its input and moves uniquely typed elements.
// No pointer inequality, buffer reuse or Rust IntoIter rule is assumed.
const PRODUCER: &str = r#"
fn need[T](take x:unique T) {}
fn relay[T](take source:unique List[unique T]) {
  for x in take source {need(take x)}
}
fn verify(source:List[List[String]], expected:u64)->bool {
  let snapshot=copy source
  let mut rows:unique List[unique List[String]]=unique source
  if list.len(rows)!=expected {return false}
  if expected>0 {
    list.push(mut rows[0],f"changed")
    if list.len(rows[0])!=list.len(snapshot[0])+1 {return false}
  }
  snapshot==source
}
pub fn main()->Result[(),()] {
  let owned:unique List[unique List[String]]=[[f"owned"]]
  relay(take owned)
  relay({let saved:unique List[unique String]=["a"];take saved})
  if !verify([],0) {return Error(())}
  if !verify([[f"a"],[f"b"]],2) {return Error(())}
  if !verify([[f"a"]],1) {return Error(())}
  if !verify([[f"a"],[f"b"],[f"c"]],3) {return Error(())}
  Ok(())
}
"#;

#[test]
fn generic_element_context_21_producer_snapshots_execute() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.hir");
        std::fs::write(&path, PRODUCER).unwrap();
        let mut program = load_program(&path).unwrap();
        if let Err(error) = program.check_for_execution() {
            panic!("{}", error);
        }
        let execution = run_observed_checked(
            &mut program.engine,
            &RunConfiguration {
                entry: program.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let reference = native::Observation::of_reference(execution.observation.as_ref().unwrap());
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
        let scratch = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("generic-element-context-21");
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
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
        }
    });
}
