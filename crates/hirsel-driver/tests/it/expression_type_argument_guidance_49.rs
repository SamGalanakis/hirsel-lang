//! Diagnostic ranges and accepted twins adapt rust-analyzer's
//! ide-diagnostics/src/handlers/generic_args_prohibited.rs:14–70,80–140,186–247.
//! Hirsel prohibits expression generics; deleting brackets is not a general fix.
use hirsel_driver::*;
use hirsel_engine::*;

const HOLDER: &str = "type Holder[T] { item:T }\n";
const ID: &str = "fn id[T](take x:T)->T {take x}\n";
const MESSAGE: &str = "type arguments are never written in expressions";

fn source(header: &str, body: &str) -> String {
    format!("{header}pub fn main() {{{body}}}\n")
}

fn check(text: &str) -> (QueryEngine, CheckOutput) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("expression-generics.hir", text.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    (engine, output)
}

fn accepted(text: &str) {
    let (mut engine, output) = check(text);
    assert!(
        output.diagnostics.is_empty(),
        "{text}\n{:?}",
        output.diagnostics
    );
    engine
        .lower_to_tir(&entry_path(&default_context()))
        .unwrap();
    let input = engine.export_native_input().unwrap();
    engine.import_native_input(input).unwrap();
}

fn rejected(text: &str, primary: &str) {
    rejected_with_context(text, primary, &[]);
}

fn rejected_with_context(text: &str, primary: &str, other_errors: &[(&str, &str)]) {
    let (mut engine, output) = check(text);
    assert_eq!(
        output.diagnostics.len(),
        1 + other_errors.len(),
        "{text}\n{:?}",
        output.diagnostics
    );
    let d = output
        .diagnostics
        .iter()
        .find(|d| d.message == MESSAGE)
        .unwrap_or_else(|| panic!("{text}\n{:?}", output.diagnostics));
    for (message, primary) in other_errors {
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "LANG-2"
                    && d.message == *message
                    && output.sources.slice(d.primary) == Some(*primary)),
            "{text}\n{:?}",
            output.diagnostics
        );
    }
    assert_eq!(d.rule.as_str(), "LANG-2");
    assert_eq!(d.message, MESSAGE);
    assert_eq!(output.sources.slice(d.primary), Some(primary));
    let start = text.rfind(primary).unwrap();
    let span = output.sources.resolve(d.primary);
    assert_eq!(
        (span.start as usize, span.end as usize),
        (start, start + primary.len())
    );
    assert!(
        d.fix.is_none(),
        "a bracket deletion is not a general repair"
    );
    let human = d.render_human(&output.sources);
    let json = d.to_json(&output.sources);
    assert_eq!(json["rule"], "LANG-2");
    assert_eq!(json["message"], MESSAGE);
    assert_eq!(json["primary"]["start"], start);
    assert_eq!(json["primary"]["end"], start + primary.len());
    assert!(json["fix"].is_null());
    assert!(d.notes.iter().map(String::len).sum::<usize>() < 1024);
    assert!(human.len() < 4096);
    assert!(json.to_string().len() < 4096);
    assert_eq!(json["notes"].as_array().unwrap().len(), d.notes.len());
    for (note, rendered) in d.notes.iter().zip(json["notes"].as_array().unwrap()) {
        assert_eq!(rendered, note);
        assert!(human.contains(note), "{human}");
    }
    for required in [
        "indexes values",
        "declared signatures",
        "already typed arguments",
        "expected types",
        "let value:Holder[u64]=Holder(item:1u64)",
        "id(1u64)",
        "let value:u64=id(1)",
        "let argument:u64=1; consume(argument)",
        "Later arguments never",
    ] {
        assert!(human.contains(required), "missing {required}: {human}");
    }
    assert!(!human.contains("qualify its constructor"));
    let file = span.file;
    let warm = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(warm.completion, CheckCompletion::Complete);
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
}

#[test]
fn expression_type_argument_guidance_49_constructor_and_function() {
    with_compiler_stack(|| {
        for (header, body, primary, repair) in [
            (
                HOLDER,
                "let value=Holder[u64](item:1u64)",
                "Holder[u64]",
                "let value:Holder[u64]=Holder(item:1u64)",
            ),
            (
                ID,
                "let value=id[u64](1u64)",
                "id[u64]",
                "let value=id(1u64)",
            ),
        ] {
            let text = source(header, body);
            rejected(&text, primary);
            accepted(&source(header, repair));
        }
    });
}

#[test]
fn expression_type_argument_guidance_49_nested_type() {
    with_compiler_stack(|| {
        for (header, body, repair, other_error) in [
            (
                HOLDER,
                "let argument:List[u64]=[1u64];let value=Holder[List[u64]](item:take argument)",
                "let argument:List[u64]=[1u64];let value:Holder[List[u64]]=Holder(item:take argument)",
                (
                    "a bare generic constructor requires an expected type",
                    "Holder",
                ),
            ),
            (
                ID,
                "let argument:List[u64]=[1u64];let value=id[List[u64]](take argument)",
                "let argument:List[u64]=[1u64];let value=id(take argument)",
                (
                    "generic function value cannot determine parameter `T` from its immediate complete expected callable type",
                    "id",
                ),
            ),
        ] {
            // Nested brackets already report the bare generic base as well as
            // the inner index. Preserve both producers and their source ranges.
            rejected_with_context(&source(header, body), "List[u64]", &[other_error]);
            accepted(&source(header, repair));
        }
    });
}

#[test]
fn expression_type_argument_guidance_49_named_and_parameter_types() {
    with_compiler_stack(|| {
        for (header, body, primary, repair) in [
            (
                format!("{HOLDER}type Payload {{ item:u64 }}\n"),
                "let value=Holder[Payload](item:Payload(item:1u64))",
                "Holder[Payload]",
                "let value:Holder[Payload]=Holder(item:Payload(item:1u64))",
            ),
            (
                format!("{ID}type Payload {{ item:u64 }}\n"),
                "let value=id[Payload](Payload(item:1u64))",
                "id[Payload]",
                "let argument:Payload=Payload(item:1u64);let value=id(argument)",
            ),
        ] {
            rejected(&source(&header, body), primary);
            accepted(&source(&header, repair));
        }
        for (header, primary, bad, good, body) in [
            (
                ID,
                "id[T]",
                "fn relay[T](take x:T)->T {id[T](take x)}\n",
                "fn relay[T](take x:T)->T {id(take x)}\n",
                "let value=relay(1u64)",
            ),
            (
                HOLDER,
                "Holder[T]",
                "fn wrap[T](take x:T)->Holder[T] {Holder[T](item:take x)}\n",
                "fn wrap[T](take x:T)->Holder[T] {let value:Holder[T]=Holder(item:take x);take value}\n",
                "let value=wrap(1u64)",
            ),
        ] {
            rejected(&source(&format!("{header}{bad}"), body), primary);
            accepted(&source(&format!("{header}{good}"), body));
        }
    });
}

#[test]
fn expression_type_argument_guidance_49_absent_context_and_applied_recipes() {
    with_compiler_stack(|| {
        for (header, body, primary, repair) in [
            (
                HOLDER,
                "let value=Holder[u64](item:1)",
                "Holder[u64]",
                "let value:Holder[u64]=Holder(item:1)",
            ),
            (ID, "let value=id[u64](1)", "id[u64]", "let value:u64=id(1)"),
            (
                "fn consume[T](x:T) {}\n",
                "consume[u64](1)",
                "consume[u64]",
                "let argument:u64=1; consume(argument)",
            ),
            (
                "type Empty[T] {}\n",
                "let value=Empty[u64]()",
                "Empty[u64]",
                "let value:Empty[u64]=Empty()",
            ),
        ] {
            rejected(&source(header, body), primary);
            // Apply the annotation recipe to the full program, rather than
            // treating the rejected brackets as a safe standalone deletion.
            accepted(&source(header, body).replace(body, repair));
            let (_, output) = check(&source(header, body.replace("[u64]", "").as_str()));
            assert!(
                !output.diagnostics.is_empty(),
                "deletion unexpectedly sufficient: {body}"
            );
            assert!(output.diagnostics.iter().all(|d| d.fix.is_none()));
        }
    });
}

#[test]
fn expression_type_argument_guidance_49_prefix_and_unrelated_fallback() {
    with_compiler_stack(|| {
        let header = "fn consume[T](first:T, second:T) {}\n";
        let (_, output) = check(&source(header, "consume(1,2u64)"));
        assert!(
            output.diagnostics.iter().any(
                |d| d.rule.as_str() == "LANG-2" && output.sources.slice(d.primary) == Some("1")
            )
        );
        assert!(
            output
                .diagnostics
                .iter()
                .all(|d| !d.notes.iter().any(|n| n.contains("postfix")) && d.fix.is_none())
        );
        accepted(&source(header, "let first:u64=1;consume(first,2u64)"));
        // LANG-2 producers outside expression brackets retain global guidance.
        let (_, output) = check(&source("", "let value=1"));
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "LANG-2");
        assert!(
            d.render_human(&output.sources)
                .contains("qualify its constructor")
        );
        assert!(
            d.to_json(&output.sources)["notes"]
                .to_string()
                .contains("qualify its constructor")
        );
    });
}

#[test]
fn expression_type_argument_guidance_49_real_indexing_and_same_spelled_values() {
    with_compiler_stack(|| {
        for (header, body) in [
            (
                "",
                "let xs:List[u64]=[7u64];let index=0u64;let value=xs[index]",
            ),
            (
                "type Offset { item:u64 }\n",
                "let Offset=0u64;let xs:List[u64]=[7u64];let value=xs[Offset]",
            ),
            ("", "let u64=0u64;let xs:List[u64]=[7u64];let value=xs[u64]"),
            (
                "fn at[T](xs:List[T], T:u64) {let _=xs[T]}\n",
                "let xs:List[u64]=[7u64];let value=at(xs,0u64)",
            ),
            (
                "fn last[n:size](xs:Inline[u64,n])->u64 {xs[n-1u64]}\n",
                "let xs:Inline[u64,1]=[7u64];let value=last(xs)",
            ),
            (
                "fn choose(xs:List[fn(u64)->u64], index:u64)->u64 {xs[index](1u64)}\nfn one(x:u64)->u64 {x}\n",
                "let xs:List[fn(u64)->u64]=[one];let value=choose(xs,0u64)",
            ),
        ] {
            accepted(&source(header, body));
        }
        let (_, output) = check(&source("", "let xs:List[u64]=[7u64];let value=xs[true]"));
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "LANG-7")
        );
        assert!(
            output
                .diagnostics
                .iter()
                .all(|d| d.message != MESSAGE && !d.notes.iter().any(|n| n.contains("postfix")))
        );
        let (_, output) = check(&source(ID, "let value=id[0u64](1u64)"));
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "LANG-2");
        assert_eq!(
            d.message,
            "generic function value cannot determine parameter `T` from its immediate complete expected callable type"
        );
        assert_eq!(output.sources.slice(d.primary), Some("id"));
        assert!(d.notes.iter().all(|n| !n.contains("postfix")));
        let (_, output) = check(&source(
            "fn concrete(x:u64)->u64 {x}\n",
            "let value=concrete[0u64](1u64)",
        ));
        assert!(output.diagnostics.iter().any(|d| d.rule.as_str() == "LANG-7"
            && d.message == "indexing requires a sequence"));
        assert!(
            output
                .diagnostics
                .iter()
                .all(|d| d.message != MESSAGE && !d.notes.iter().any(|n| n.contains("postfix")))
        );
    });
}

#[test]
fn expression_type_argument_guidance_49_utf8_ranges_and_repairs() {
    with_compiler_stack(|| {
        for (header, expression, repair) in [
            (
                HOLDER,
                "Holder[u64](item:1u64)",
                "let value:Holder[u64]=Holder(item:1u64)",
            ),
            (ID, "id[u64](1u64)", "let value:u64=id(1u64)"),
        ] {
            let body = format!("let text=\"λ🐑\";let value={expression}");
            let text = source(&format!("// café 🐑\n{header}"), &body);
            let primary = expression.split('(').next().unwrap();
            rejected(&text, primary);
            accepted(&text.replace(&format!("let value={expression}"), repair));
        }
    });
}
