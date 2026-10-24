//! Pattern/source ranges follow rust-analyzer mutable_ref.rs:24-62; all
//! acceptance and consuming-match expectations are Hirsel LANG-13/OWN-1.
use hirsel_driver::*;
use hirsel_engine::*;

fn checked(source: &str) -> CheckOutput {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("match.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    output
}

fn own_error(output: &CheckOutput) -> &hirsel_diag::Diagnostic {
    let errors: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    let [d] = errors[..] else {
        panic!("{:?}", output.diagnostics)
    };
    assert_eq!(d.rule.as_str(), "OWN-1", "{d:?}");
    d
}

fn has_recipe(d: &hirsel_diag::Diagnostic, owner: &str) -> bool {
    d.notes
        .iter()
        .any(|n| n.contains(&format!("`match take {owner}`")))
}

fn label(output: &CheckOutput, needle: &str, source: &str) {
    let d = own_error(output);
    assert!(
        d.secondary
            .iter()
            .any(|l| l.message.contains(needle) && output.sources.slice(l.span) == Some(source)),
        "{}",
        d.render_human(&output.sources)
    );
}

#[test]
fn borrowed_match_guidance_49_minimal_utf8_ranges_and_json() {
    with_compiler_stack(|| {
        for prefix in ["", "// λ 🐑\n"] {
            let source = format!(
                "{prefix}fn extract(take value:Option[String])->String {{ match value {{ Some(text)=>take text, None()=>\"\" }} }}"
            );
            let output = checked(&source);
            let d = own_error(&output);
            assert_eq!(output.sources.slice(d.primary), Some("take text"));
            assert_eq!(
                output.sources.resolve(d.primary).start as usize,
                source.find("take text").unwrap()
            );
            label(&output, "pattern", "text");
            label(&output, "inspects", "match value");
            assert!(
                has_recipe(d, "value"),
                "{}",
                d.render_human(&output.sources)
            );
            assert!(d.fix.is_none());
            let help = d
                .notes
                .iter()
                .find(|n| n.contains("`match take value`"))
                .unwrap();
            for word in ["if", "whole", "later", "drop"] {
                assert!(help.contains(word), "{help}");
            }
            let json = d.to_json(&output.sources);
            let human = d.render_human(&output.sources);
            assert_eq!(json["primary"]["start"], source.find("take text").unwrap());
            for (i, l) in d.secondary.iter().enumerate() {
                assert_eq!(json["secondary"][i]["message"], l.message);
                assert_eq!(
                    json["secondary"][i]["span"]["start"],
                    output.sources.resolve(l.span).start
                );
                assert!(human.contains(&l.message), "{human}");
            }
            for note in &d.notes {
                assert!(json["notes"].as_array().unwrap().iter().any(|n| n == note));
                assert!(
                    human.contains(note.strip_prefix("help: ").unwrap_or(note)),
                    "{human}"
                );
            }
        }
    });
}

#[test]
fn borrowed_match_guidance_49_enum_owners_and_later_use_are_conditional() {
    with_compiler_stack(|| {
        for (declaration, ty, initializer, arms) in [
            (
                "",
                "Option[String]",
                "let value: Option[String] = Some(\"yes\")",
                "Some(text)=>take text, None()=>\"\"",
            ),
            (
                "",
                "Result[String, ()]",
                "let value: Result[String, ()] = Ok(\"yes\")",
                "Ok(text)=>take text, Error(_)=>\"\"",
            ),
            (
                "type Message = Text(String) | Empty",
                "Message",
                "let value = Text(\"yes\")",
                "Text(text)=>take text, Empty()=>\"\"",
            ),
        ] {
            for (parameters, initialization, recipe) in [
                (String::new(), format!("{initializer};"), true),
                (format!("take value:{ty}"), String::new(), true),
                (format!("value:{ty}"), String::new(), false),
                (format!("mut value:{ty}"), String::new(), false),
            ] {
                let source = format!(
                    "{declaration}\nfn extract({parameters})->String {{ {initialization} match value {{ {arms} }} }}"
                );
                let output = checked(&source);
                assert_eq!(
                    has_recipe(own_error(&output), "value"),
                    recipe,
                    "{source}: {:?}",
                    output.diagnostics
                );
                label(&output, "inspects", "match value");
            }
        }
        let output = checked(
            "fn extract(take value:Option[String])->String { let extracted = match value { Some(text)=>take text, None()=>\"\" }; let _ = match value { Some(later)=>string.byte_len(later), None()=>0 }; extracted }",
        );
        assert!(has_recipe(own_error(&output), "value"));
        // The note is conditional: blindly consuming this owner changes the
        // existing later use into an availability error.
        let invalid = checked(
            "fn extract(take value:Option[String])->String { let extracted = match take value { Some(text)=>take text, None()=>\"\" }; let _ = match value { Some(later)=>string.byte_len(later), None()=>0 }; extracted }",
        );
        assert!(
            invalid
                .diagnostics
                .iter()
                .any(|d| d.severity == hirsel_diag::Severity::Error)
        );
    });
}

#[test]
fn borrowed_match_guidance_49_nested_match_roots_and_aliases() {
    with_compiler_stack(|| {
        let output = checked(
            "fn extract(take value:Option[Option[String]])->String { match value { Some(inner)=> match inner { Some(text)=>take text, None()=>\"\" }, None()=>\"\" } }",
        );
        label(&output, "inspects", "match inner");
        label(&output, "inspects", "match value");
        assert!(!has_recipe(own_error(&output), "value"));
        assert!(!has_recipe(own_error(&output), "inner"));
        assert!(
            own_error(&output)
                .notes
                .iter()
                .any(|n| n.contains("outer owner alone"))
        );
        // Once the outer match has actually transferred the enum, the
        // inner owner is proved owned and has its own consuming recipe.
        let output = checked(
            "fn extract(take value:Option[Option[String]])->String { match take value { Some(inner)=> match inner { Some(text)=>take text, None()=>\"\" }, None()=>\"\" } }",
        );
        label(&output, "inspects", "match inner");
        assert!(has_recipe(own_error(&output), "inner"));
        assert!(!has_recipe(own_error(&output), "value"));
        let output = checked(
            "fn extract(take value:Option[String])->String { match value { Some(text)=> { let alias = text; take alias }, None()=>\"\" } }",
        );
        // Even an alias must not manufacture ownership of the payload.
        assert!(own_error(&output).fix.is_none());
    });
}

#[test]
fn borrowed_match_guidance_49_no_false_consuming_owner_recipe() {
    with_compiler_stack(|| {
        for source in [
            "fn extract(value:Option[String])->String { match value { Some(text)=>take text, None()=>\"\" } }",
            "fn extract(mut value:Option[String])->String { match value { Some(text)=>take text, None()=>\"\" } }",
            "fn extract(take value:Option[String])->String { let callback = fn()->String { match value { Some(text)=>take text, None()=>\"\" } }; callback() }",
            "resource type Item { number:u64 }\ndrop(item) {}\ntype Holder { value:Option[Item] }\nfn extract(take holder:Holder)->Item { match holder.value { Some(text)=>take text, None()=>Item(number:0) } }",
            "resource type Item { name:String }\ndrop(item) {}\nfn extract(take value:Option[Item])->Item { match value { Some(text)=>take text, None()=>Item(name:\"\") } }",
            "resource type Message = Text(String) | Empty\ndrop(message) {}\nfn extract(take value:Message)->String { match value { Text(text)=>take text, Empty()=>\"\" } }",
            "fn extract[T](take value:Option[T])->() { match value { Some(text)=>{ let _ = take text }, None()=>() } }",
            "fn extract(take value:Option[String])->String { let borrow = fn()->u64 { match value { Some(text)=>string.byte_len(text), None()=>0 } }; let extracted = match value { Some(text)=>take text, None()=>\"\" }; let _ = borrow(); extracted }",
            "fn extract(take value:Option[String])->String { let borrow = fn()->u64 { match value { Some(text)=>string.byte_len(text), None()=>0 } }; match value { Some(text) if borrow()>0=>take text, _=>\"\" } }",
            "fn extract(take value:Option[String]) { while true { match value { Some(text)=>{ let _ = take text }, None()=>() } } }",
            "fn extract(take value:Option[String]) { while let Some(text) = value { let _ = take text } }",
            "fn extract(take value:Option[String]) { if let Some(text) = value { let _ = take text } }",
        ] {
            let output = checked(source);
            assert!(
                !output.diagnostics.is_empty(),
                "expected rejection: {source}"
            );
            let d = own_error(&output);
            assert!(d.fix.is_none(), "{source}: {d:?}");
            assert!(
                !d.notes.iter().any(|n| n.contains("`match take ")),
                "{source}: {}",
                d.render_human(&output.sources)
            );
        }
    });
}

#[test]
fn borrowed_match_guidance_49_guard_and_early_return_context() {
    with_compiler_stack(|| {
        for body in [
            "match value { Some(text) if string.byte_len(text)>0 =>take text, _=>\"\" }",
            "match value { Some(text)=>return take text, None()=>return \"\" }",
        ] {
            let output = checked(&format!(
                "fn extract(take value:Option[String])->String {{ {body} }}"
            ));
            label(&output, "pattern", "text");
            label(&output, "inspects", "match value");
            assert!(
                has_recipe(own_error(&output), "value"),
                "{:?}",
                output.diagnostics
            );
        }
    });
}

// Independently written source/V twins. Every function is reachable from main;
// no edit is emitted, and these oracles exercise both outcomes, guards, early
// returns, nested matches and resource cleanup without a deep copy.
const TWINS: &str = r#"
type Message = Text(String) | Empty
resource type Item { number:u64 }
drop(item) {}
fn extract(take value:Option[String])->String {
  match take value { Some(text)=>take text, None()=>"" }
}
fn from_result(take value:Result[String,()])->String {
  match take value { Ok(text)=>take text, Error(_)=>"" }
}
fn user(take value:Message)->String {
  match take value { Text(text)=>take text, Empty()=>"" }
}
fn from_local()->String {
  let value:Option[String] = Some("local")
  match take value { Some(text)=>take text, None()=>"" }
}
fn nested(take value:Option[Option[String]])->String {
  match take value {
    Some(inner)=>match take inner { Some(text)=>take text, None()=>"" }
    None()=>""
  }
}
fn guarded(take value:Option[String])->String {
  match take value { Some(text) if string.byte_len(text)>0=>take text, _=>"" }
}
fn early(take value:Option[String])->String {
  match take value { Some(text)=>return take text, None()=>return "" }
}
fn from_resource(take value:Option[Item])->u64 {
  match take value { Some(item)=>item.number, None()=>0 }
}
pub fn main()->Result[(),()] {
  failure.check(extract(Some("option"))=="option", Arithmetic())
  failure.check(extract(None())=="", Arithmetic())
  failure.check(from_result(Ok("result"))=="result", Arithmetic())
  failure.check(from_result(Error(()))=="", Arithmetic())
  failure.check(user(Text("user"))=="user", Arithmetic())
  failure.check(user(Empty())=="", Arithmetic())
  failure.check(from_local()=="local", Arithmetic())
  failure.check(nested(Some(Some("nested")))=="nested", Arithmetic())
  failure.check(nested(Some(None()))=="", Arithmetic())
  failure.check(nested(None())=="", Arithmetic())
  failure.check(guarded(Some("guard"))=="guard", Arithmetic())
  failure.check(guarded(Some(""))=="", Arithmetic())
  failure.check(guarded(None())=="", Arithmetic())
  failure.check(early(Some("early"))=="early", Arithmetic())
  failure.check(early(None())=="", Arithmetic())
  failure.check(from_resource(Some(Item(number:7)))==7, Arithmetic())
  failure.check(from_resource(None())==0, Arithmetic())
  Ok(())
}
"#;

#[test]
fn borrowed_match_guidance_49_independent_twins_execute_and_validate_reachable_v() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("twins.hir", TWINS.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let execution = run_observed_checked(
            &mut engine,
            &RunConfiguration {
                entry: entry_path(&default_context()),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let observation = execution.observation.as_ref().unwrap();
        let reference = native::Observation::of_reference(observation);
        assert_eq!(reference.exit_code, Some(0), "{reference:?}");
        assert!(reference.failures.is_empty(), "{reference:?}");
        assert_eq!(
            observation
                .effects
                .iter()
                .filter(|e| matches!(
                    e.effect,
                    hirsel_interp::observation::Effect::Finalizer { .. }
                ))
                .count(),
            1
        );
        let input = engine.export_native_input().unwrap();
        engine.import_native_input(input).unwrap();
    });
}
