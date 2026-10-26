//! Every declaration recipe has a checked edited twin and an unsafe witness.
use hirsel_driver::*;
use hirsel_engine::*;

fn checked(engine: &mut QueryEngine, source: &str) -> CheckOutput {
    let file = engine.sources.add("recipes.hir", source.into()).unwrap();
    check_progress(engine, file, &default_context()).unwrap()
}
fn rejected(engine: &mut QueryEngine, source: &str, recipe: &str, offered: bool) {
    let output = checked(engine, source);
    assert!(
        !output.diagnostics.is_empty(),
        "unexpected acceptance: {source}"
    );
    assert_eq!(
        output
            .diagnostics
            .iter()
            .flat_map(|d| &d.notes)
            .any(|n| n.starts_with(recipe)),
        offered,
        "{source}\n{}",
        output
            .diagnostics
            .iter()
            .map(|d| d.render_human(&output.sources))
            .collect::<Vec<_>>()
            .join("\n")
    );
    for diagnostic in output
        .diagnostics
        .iter()
        .filter(|d| d.notes.iter().any(|n| n.starts_with(recipe)))
    {
        assert!(
            diagnostic
                .secondary
                .iter()
                .any(|l| l.message == "checked declaration repair")
        );
    }
}
fn accepted(engine: &mut QueryEngine, source: &str) {
    let output = checked(engine, source);
    assert_eq!(output.completion, CheckCompletion::Complete);
    assert!(
        output.diagnostics.is_empty(),
        "{source}\n{:?}",
        output.diagnostics
    );
}

#[test]
fn mode2_recipes_52_r1_public_producer() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = "pub fn code() -> String { \"x\" }\nfn need(take s: unique String) {}\npub fn main() { need(code()) }";
        rejected(&mut engine, source, "R1 ", true);
        accepted(
            &mut engine,
            &source.replace("-> String", "-> unique String"),
        );
        rejected(
            &mut engine,
            &source
                .replace("{ \"x\" }", "{ NAME }")
                .replace("pub fn code", "const NAME: String = \"x\"\npub fn code"),
            "R1 ",
            false,
        );
    });
}

#[test]
fn mode2_recipes_52_r1_payload_header() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = "pub fn code() -> Result[String, ()] { Ok(\"x\") }\nfn need(take s: unique String) {}\npub fn main() { let Ok(s) = code() else { return }; need(take s) }";
        rejected(&mut engine, source, "R1 ", true);
        accepted(
            &mut engine,
            &source.replace("Result[String", "Result[unique String"),
        );
    });
}

#[test]
fn mode2_recipes_52_r2_generic_wrapper() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = "type Credit { tag: String }\ntype Envelope[T] { credit: Credit, payload: T }\nfn wrap[T](take c: Credit, take t: T) -> Envelope[T] { Envelope(credit: take c, payload: take t) }\nfn need(take e: unique Envelope[String]) {}\npub fn main() { need(wrap(unique Credit(tag: \"c\"), unique \"m\")) }";
        rejected(&mut engine, source, "R2 ", true);
        accepted(
            &mut engine,
            &source
                .replace("take c: Credit", "take c: unique Credit")
                .replace("take t: T", "take t: unique T"),
        );
        rejected(
            &mut engine,
            &source
                .replace("unique \"m\"", "copy NAME")
                .replace("type Credit", "const NAME: String = \"m\"\ntype Credit"),
            "R2 ",
            false,
        );
    });
}

#[test]
fn mode2_recipes_52_r3_spawn_literal() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = "fn probe(s: Scope) { let child: Task[unique Result[unique String, ()]] of s = task.async(s, fn() -> Result[String, ()] { Ok(\"x\") }); let _ = task.await(s, take child) }";
        rejected(&mut engine, source, "R3 ", true);
        accepted(
            &mut engine,
            &source.replace(
                "fn() -> Result[String",
                "fn() -> unique Result[unique String",
            ),
        );
        rejected(
            &mut engine,
            &format!(
                "const NAME: String = \"x\"\n{}",
                source.replace("Ok(\"x\")", "Ok(NAME)")
            ),
            "R3 ",
            false,
        );
    });
}

#[test]
fn mode2_recipes_52_r3_await_literal() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = "fn probe(s: Scope) -> Result[unique String, ()] { let child = task.async(s, fn() -> Result[String, ()] { Ok(\"x\") }); task.await(s, take child) }";
        rejected(&mut engine, source, "R3 ", true);
        accepted(
            &mut engine,
            &source.replace(
                "fn() -> Result[String",
                "fn() -> unique Result[unique String",
            ),
        );
        // Generic inference does not retain an uncontextualized root promise.
        // That case also needs a Task annotation, so a literal-only recipe is withheld.
        rejected(
            &mut engine,
            &source.replace(
                "fn probe(s: Scope) -> Result",
                "fn probe(s: Scope) -> unique Result",
            ),
            "R3 ",
            false,
        );
        rejected(
            &mut engine,
            &format!(
                "const NAME: String = \"x\"\n{}",
                source.replace("Ok(\"x\")", "Ok(NAME)")
            ),
            "R3 ",
            false,
        );
    });
}

#[test]
fn mode2_recipes_52_r3_select_literal() {
    with_compiler_stack(|| {
        let mut engine = crate::source_std::engine();
        let source = "fn probe(s: Scope) -> Result[unique String, ()] { let child = task.async(s, fn() -> Result[String, ()] { Ok(\"x\") }); let mut pending: TaskSet[Result[unique String, ()]] of s = task.set(s); task.add(s, mut pending, take child); let (_, row) = task.select(s, take pending); take row }";
        rejected(&mut engine, source, "R3 ", true);
        accepted(
            &mut engine,
            &source.replace(
                "fn() -> Result[String",
                "fn() -> unique Result[unique String",
            ),
        );
        rejected(
            &mut engine,
            &format!(
                "const NAME: String = \"x\"\n{}",
                source.replace("Ok(\"x\")", "Ok(NAME)")
            ),
            "R3 ",
            false,
        );
    });
}

#[test]
fn mode2_recipes_52_r4_inferred_nested_row() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = "type Sample { name: String }\ntype Strategy[T] { draw: fn() -> Result[unique T, ()] }\nfn make[T](take draw: fn() -> Result[unique T, ()]) -> Strategy[T] { Strategy(draw: take draw) }\nfn need(take s: Strategy[Sample]) {}\npub fn main() { let strategy = make(fn() -> Result[unique Sample, ()] { Ok(Sample(name: \"x\")) }); need(take strategy) }";
        rejected(&mut engine, source, "R4 ", true);
        accepted(
            &mut engine,
            &source.replace("let strategy =", "let strategy: Strategy[Sample] ="),
        );
        // Weakening an inferred row cannot repair a different payload shape.
        let negative = source.replace("take s: Strategy[Sample]", "take s: Strategy[String]");
        rejected(&mut engine, &negative, "R4 ", false);
    });
}

#[test]
fn mode2_recipes_52_r3_inferred_literal_result() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = "fn probe(s: Scope) -> Result[unique String, ()] { let child = task.async(s, fn() { if true { Ok(\"x\") } else { Error(()) } }); task.await(s, take child) }";
        rejected(&mut engine, source, "R3 ", true);
        accepted(
            &mut engine,
            &source.replace("fn() {", "fn() -> unique Result[unique String, ()] {"),
        );
        rejected(
            &mut engine,
            &format!(
                "const NAME: String = \"x\"\n{}",
                source.replace("Ok(\"x\")", "Ok(NAME)")
            ),
            "R3 ",
            false,
        );
    });
}
