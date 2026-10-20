//! Authority checking of real source, from hirsel-authority's unit tests.
use hirsel_types::*;

#[test]
fn taking_a_directory_into_a_borrowed_call_ends_its_open_files_brand() {
    use hirsel_engine::DefinitionQueries;
    for (passing, rejected) in [("take ", true), ("", false)] {
        let source = format!(
            "pub fn main(t: Test) -> Result[(), FileError] {{\nlet p = path.of(\"sub/value\")\nlet dir = test.dir(t, [(p, b\"data\")])\nlet fresh = file.open(dir, p)?\nlet sub = dir.narrow({passing}dir, path.of(\"sub\"))?\nfile.narrow(sub, take fresh)\nOk(())\n}}"
        );
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let file = engine.sources.add("directory.hir", source).unwrap();
        engine.parse_file(file, &context).unwrap();
        let diagnostics = match engine.mode_definition(&hirsel_engine::entry_path(&context)) {
            Ok(output) => output.diagnostics,
            Err(QueryError::Diagnostics(diagnostics)) => diagnostics,
            Err(error) => panic!("{error:?}"),
        };
        assert_eq!(
            diagnostics.first().map(|d| d.rule.as_str()),
            rejected.then_some("PROV-1"),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn error_mapper_occurrences_check_the_owned_payloads_receiving_modes() {
    use hirsel_engine::DefinitionQueries;
    for (header, rule) in [
        ("fn error[E](take e: unique E) {}", Some("MODE-2")),
        ("fn error[E](take e: unique sendable E) {}", Some("MODE-2")),
        ("fn error[E](take e: sendable E) {}", Some("MODE-4")),
        ("fn error[E: sendable](take e: E) {}", Some("MODE-4")),
        ("fn error[E](take e: E) {}", None),
        ("fn error[E](e: E) {}", None),
    ] {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let source = format!(
            "{header}\nfn main(take b: Bytes) -> Result[(), ()] {{\nstring.from_bytes(copy b) ? error\nOk(())\n}}"
        );
        let file = engine.sources.add("mapper.hir", source).unwrap();
        engine.parse_file(file, &context).unwrap();
        let diagnostics = match engine.mode_definition(&hirsel_engine::entry_path(&context)) {
            Ok(output) => output.diagnostics,
            Err(QueryError::Diagnostics(diagnostics)) => diagnostics,
            Err(error) => panic!("{error:?}"),
        };
        assert_eq!(
            diagnostics.first().map(|d| d.rule.as_str()),
            rule,
            "{header}: {diagnostics:?}"
        );
    }
    for result in [
        "unique Result[(), unique Bytes]",
        "unique Result[(), Bytes]",
    ] {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let source = format!(
            "fn error[E](take e: unique E) {{}}\nfn main(take r: {result}) -> Result[(), ()] {{ (take r) ? error\nOk(()) }}"
        );
        let file = engine.sources.add("unique-mapper.hir", source).unwrap();
        engine.parse_file(file, &context).unwrap();
        engine
            .lower_to_tir(&hirsel_engine::entry_path(&context))
            .unwrap();
    }
}

#[test]
fn annotated_nested_receivers_keep_their_declared_payload_contract() {
    use hirsel_engine::DefinitionQueries;
    let source = "fn inspect(x:box List[Bytes]){}\nfn probe(){let xs:List[Bytes]=[b\"x\"];let built=box unique xs;let received:box List[Bytes]=take built;inspect(received)}";
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let context = hirsel_engine::default_context();
    let file = engine.sources.add("nested.hir", source.into()).unwrap();
    engine.parse_file(file, &context).unwrap();
    let mut path = hirsel_engine::entry_path(&context);
    path.0.name = "probe".into();
    let output = engine.mode_definition(&path).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = &output.value.typed.checked;
    let call = checked.calls.first().unwrap();
    let argument = call.arguments[0].expression;
    let actual = checked
        .expressions
        .iter()
        .find(|row| row.expression == argument)
        .unwrap()
        .ty;
    let TypeKind::Box(payload) = checked.types.get(actual).unwrap().kind else {
        panic!()
    };
    assert_eq!(
        checked.types.get(payload).unwrap().modes.uniqueness,
        Uniqueness::Aliased
    );
}
