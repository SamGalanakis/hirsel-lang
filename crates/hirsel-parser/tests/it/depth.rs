use crate::support;
use hirsel_parser::MAX_PARSE_DEPTH;

mod shapes {
    include!("../support/depth.rs");
}
use shapes::program;

#[test]
fn every_recursive_and_loop_built_chain_has_the_same_boundary() {
    support::with_compiler_stack(|| {
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
            let (accepted, _) = support::parse(&program(kind, MAX_PARSE_DEPTH));
            assert!(
                accepted.nesting_limit.is_none(),
                "{kind} at limit: {:?}",
                accepted.nesting_limit
            );
            assert!(
                accepted.diagnostics.is_empty(),
                "{kind}: {:?}",
                accepted.diagnostics
            );
            let (limited, _) = support::parse(&program(kind, MAX_PARSE_DEPTH + 1));
            assert!(limited.nesting_limit.is_some(), "{kind} at limit plus one");
            assert!(limited.syntax.has_syntax_errors, "{kind}");
        }
    });
}
