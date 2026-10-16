use hirsel_engine::{DefinitionQueries, QueryEngine};

#[test]
fn indexed_loop_patterns_follow_the_published_source_storage() {
    hirsel_engine::with_compiler_stack(|| {
        for (element, literal, pattern, read) in [
            ("String", "\"a\"", "input", "input == input"),
            (
                "(String, String)",
                "(\"a\", \"a\")",
                "(input, expected)",
                "input == expected",
            ),
            (
                "((String, u64), String)",
                "((\"a\", 1u64), \"a\")",
                "((input, n), expected)",
                "input == expected && n == 1u64",
            ),
            (
                "Entry",
                "Entry(text: \"a\", n: 1u64)",
                "Entry(text: input, n: n)",
                "input == input && n == 1u64",
            ),
        ] {
            for sequence in ["Array", "List", "Inline"] {
                let ty = if sequence == "Inline" {
                    format!("Inline[{element}, 1]")
                } else {
                    format!("{sequence}[{element}]")
                };
                for relation in ["", " in names", " in parents"] {
                    for marker in ["", "mut ", "take "] {
                        let source = format!(
                            "type Entry {{ text: String, n: u64 }}\nfn main() {{ let parents: {ty} = [{literal}]; let mut names: {ty}{relation} = [{literal}]; for {pattern} in {marker}names {{ let _ = {read} }} }}"
                        );
                        let mut engine = QueryEngine::new(
                            hirsel_engine::default_environment(),
                            hirsel_driver::compiler_adapters(),
                        );
                        let context = hirsel_engine::default_context();
                        let file = engine
                            .sources
                            .add("loop-projections.hir", source.clone())
                            .unwrap();
                        let parsed = engine.parse_file(file, &context).unwrap();
                        assert!(
                            parsed.diagnostics.is_empty(),
                            "{source}\n{:?}",
                            parsed.diagnostics
                        );
                        let lowered = engine
                            .lower_to_tir(&hirsel_engine::entry_path(&context))
                            .unwrap_or_else(|error| panic!("{source}\n{error:?}"));
                        assert!(
                            lowered.diagnostics.is_empty(),
                            "{source}\n{:?}",
                            lowered.diagnostics
                        );
                    }
                }
            }
        }
    });
}
