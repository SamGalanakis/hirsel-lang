//! Inventory of previously caution-only locality rows (v22 spelling):
//! borrowed parameter -> ResultGlobal, AnnotatedGlobal, GlobalArgument;
//! mut parameter -> MutStorage, GlobalBinding, AnnotatedGlobal,
//! EscapingCapture, GlobalArgument.
//!
//! Each probe checks the rejected copy, the declaration changed to `take v`,
//! and the body changed to `unique v`. A `take v` expression with a borrowed
//! or mut parameter is separately rejected: the help must name the declaration.
use hirsel_driver::*;
use hirsel_engine::*;

fn diagnostics(source: &str) -> Vec<hirsel_diag::Diagnostic> {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("repair-probe.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context())
        .unwrap_or_else(|error| panic!("{source}\n{error:?}"));
    assert_eq!(output.completion, CheckCompletion::Complete, "{source}");
    output.diagnostics
}

#[test]
fn every_caution_only_locality_row_has_source_checked_candidates() {
    with_compiler_stack(|| {
        for (row, parameter, template) in [
            (
                "borrowed -> ResultGlobal",
                "v",
                "fn f(PARAM:Bytes)->Bytes{VALUE}",
            ),
            (
                "borrowed -> AnnotatedGlobal",
                "v",
                "fn f(PARAM:Bytes){let dst:Bytes in heap=VALUE}",
            ),
            (
                "borrowed -> GlobalArgument",
                "v",
                "fn consume(take x:Bytes){}\nfn f(PARAM:Bytes){consume(VALUE)}",
            ),
            (
                "mut -> MutStorage",
                "mut v",
                "fn f(PARAM:Bytes,mut dst:Bytes){dst=VALUE}",
            ),
            (
                "mut -> GlobalBinding",
                "mut v",
                "fn fresh()->Bytes{b\"fresh\"}\nfn f(PARAM:Bytes)->Bytes{let mut dst=fresh();dst=VALUE;dst}",
            ),
            (
                "mut -> AnnotatedGlobal",
                "mut v",
                "fn f(PARAM:Bytes){let dst:Bytes in heap=VALUE}",
            ),
            (
                "mut -> EscapingCapture",
                "mut v",
                "fn use_bytes(x:Bytes){}\ntype H{action:fn()}\nfn f(PARAM:Bytes)->H{let x=VALUE;H(action:fn(){use_bytes(x)})}",
            ),
            (
                "mut -> GlobalArgument",
                "mut v",
                "fn consume(take x:Bytes){}\nfn f(PARAM:Bytes){consume(VALUE)}",
            ),
        ] {
            let source = |parameter: &str, value: &str| {
                template.replace("PARAM", parameter).replace("VALUE", value)
            };
            let original = source(parameter, "copy v");
            let rejected = diagnostics(&original);
            assert!(!rejected.is_empty(), "{row}: {original}");
            assert!(
                rejected.iter().any(|d| d
                    .notes
                    .iter()
                    .any(|n| n.starts_with("`copy v` does not help"))),
                "{row}: {rejected:?}"
            );
            for candidate in [source("take v", "copy v"), source(parameter, "unique v")] {
                assert!(diagnostics(&candidate).is_empty(), "{row}: {candidate}");
            }
            assert_eq!(
                rejected[0].rule.as_str(),
                if row.ends_with("EscapingCapture") {
                    "CLO-4"
                } else {
                    "MODE-1"
                },
                "{row}: {rejected:?}"
            );
            for help in ["declare the parameter `take v`", "`unique v`"] {
                assert!(
                    rejected.iter().any(|d| d
                        .notes
                        .iter()
                        .any(|n| n.starts_with("help: ") && n.contains(help))),
                    "{row}: missing {help}: {rejected:?}"
                );
            }
        }
    });
}

#[test]
fn taking_a_parameter_is_a_signature_repair_and_not_a_borrowed_move() {
    with_compiler_stack(|| {
        for parameter in ["v", "mut v"] {
            let source = format!("fn f({parameter}:Bytes)->Bytes{{take v}}");
            let rejected = diagnostics(&source);
            assert!(
                rejected.iter().any(|d| d.rule.as_str() == "OWN-1"),
                "{source}: {rejected:?}"
            );
        }
        assert!(diagnostics("fn f(take v:Bytes)->Bytes{take v}").is_empty());
        let rejected = diagnostics("fn f(v:Bytes)->Bytes{v}");
        assert!(
            rejected[0]
                .notes
                .iter()
                .any(|n| n.starts_with("help: ") && n.contains("declare the parameter `take v`")),
            "{rejected:?}"
        );
        assert!(diagnostics("fn f(take v:Bytes)->Bytes{v}").is_empty());
    });
}
