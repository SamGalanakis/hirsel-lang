use hirsel_driver::*;
use hirsel_engine::*;
mod shapes {
    include!("../../../hirsel-parser/tests/support/depth.rs");
}

#[test]
fn parser_limits_are_deterministic_failures_without_published_items() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = shapes::program("binary", hirsel_parser::MAX_PARSE_DEPTH + 1);
        let file = engine.sources.add("depth.hir", source).unwrap();
        let first = engine.parse_file(file, &default_context()).unwrap_err();
        let second = engine.parse_file(file, &default_context()).unwrap_err();
        assert_eq!(first, second);
        assert!(
            matches!(first, QueryError::Operational(ref e) if e.operation == "parser-tooling-limit")
        );
        assert!(
            engine
                .definition_items(&entry_path(&default_context()))
                .is_err()
        );
    });
}

#[test]
fn all_accepted_depth_boundaries_format_without_a_second_limit() {
    with_compiler_stack(|| {
        for kind in [
            "binary",
            "call",
            "index",
            "field",
            "try",
            "unary",
            "group",
            "block",
            "tuple",
            "sequence",
            "match",
            "else-if",
            "closure",
            "if",
            "while",
            "for",
            "local-fn",
            "type-box",
            "type-function",
            "type-argument",
            "pattern-tuple",
            "pattern-constructor",
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let source = shapes::program(kind, hirsel_parser::MAX_PARSE_DEPTH);
            let file = engine.sources.add("depth.hir", source).unwrap();
            let formatted = format_snapshot(&mut engine, file, &default_context())
                .unwrap_or_else(|e| panic!("{kind}: {e:?}"));
            let file = engine
                .sources
                .add("formatted.hir", formatted.clone())
                .unwrap();
            let second = format_snapshot(&mut engine, file, &default_context())
                .unwrap_or_else(|e| panic!("{kind} formatted: {e:?}"));
            assert_eq!(formatted, second, "{kind}");
        }
    });
}

#[test]
fn ordinary_generated_chains_above_the_old_guard_elaborate() {
    with_compiler_stack(|| {
        for source in [
            format!(
                "fn f(a: u8) -> u8 {{ {} }}",
                std::iter::repeat_n("a", 130)
                    .collect::<Vec<_>>()
                    .join(" + ")
            ),
            format!("fn f(a: bool) -> bool {{ {}a }}", "! ".repeat(130)),
            format!(
                "fn f(a: u8) -> u8 {{ {}a{} }}",
                "{".repeat(130),
                "}".repeat(130)
            ),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("depth.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let path = &parsed.value.items[0].item.definition;
            let analyzed = engine.mode_definition(path).unwrap();
            assert!(
                analyzed.diagnostics.is_empty(),
                "{:?}",
                analyzed.diagnostics
            );
        }
    });
}
