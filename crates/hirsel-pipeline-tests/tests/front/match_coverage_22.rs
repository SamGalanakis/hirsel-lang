//! Source coverage/reachability checked against concrete plain values.
#[path = "../../../hirsel-validate/tests/it/match_coverage_22/model.rs"]
mod model;
use crate::check_support::path;
use hirsel_engine::{DefinitionQueries, QueryEngine};
use hirsel_syntax::FileContext;
use hirsel_types::QueryError;

#[test]
fn match_coverage_22_finite_source_truth_tables() {
    let mut counts = [0; 3];
    let cases = model::selected_cases();
    let count = cases.len();
    for case in cases {
        let truth = case.truth();
        assert!(case.arms.iter().all(|a| a.pattern.expanded().len() <= 256));
        let source = case.source();
        let mut engine = QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let file = engine
            .sources
            .add("match-coverage-22.hir", source.clone())
            .unwrap();
        let parsed = engine
            .parse_file(
                file,
                &FileContext {
                    package: path("main").0.package,
                    modules: vec![],
                },
            )
            .unwrap();
        assert!(
            parsed.diagnostics.is_empty(),
            "{}\n{source}\n{:?}",
            case.name,
            parsed.diagnostics
        );
        let ds = match engine.check_definition(&path("main")) {
            Ok(checked) => {
                let dead_catch_alls = case
                    .arms
                    .iter()
                    .zip(&truth.reachable_arms)
                    .filter(|(a, reachable)| {
                        matches!(a.pattern, model::Pattern::Wild) && !**reachable
                    })
                    .count();
                assert_eq!(
                    checked.value.plans.unreachable_arms.len(),
                    dead_catch_alls,
                    "{}",
                    case.name
                );
                checked.diagnostics.clone()
            }
            Err(QueryError::Diagnostics(ds)) => ds,
            Err(e) => panic!("{}\n{source}\n{e:?}", case.name),
        };
        assert_eq!(
            ds.iter()
                .any(|d| d.message == "match does not cover every value"),
            !truth.exhaustive,
            "{}\n{source}\n{ds:?}",
            case.name
        );
        assert_eq!(
            ds.iter()
                .filter(|d| d.message.starts_with("pattern alternative can never match"))
                .count(),
            truth.dead_alternatives,
            "{}\n{source}\n{ds:?}",
            case.name
        );
        assert_eq!(
            ds.len(),
            usize::from(!truth.exhaustive) + truth.dead_alternatives,
            "{}\n{source}\n{ds:?}",
            case.name
        );
        if ds.is_empty() {
            let lowered = engine.lower_to_tir(&path("main")).unwrap().value;
            let signature = engine.item_signature(&path("main")).unwrap().value;
            let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
            signatures.insert(signature.definition, (*signature).clone());
            if !case.domain.declaration().is_empty() {
                let signature = engine.item_signature(&path("Choice")).unwrap().value;
                signatures.insert(signature.definition, (*signature).clone());
            }
            hirsel_validate::assert_valid(&lowered, engine.types(), &signatures);
            counts[0] += 1;
        } else if !truth.exhaustive {
            counts[1] += 1;
        } else {
            counts[2] += 1;
        }
        // The catch-all in template 6 is covered but stays source-accepted.
        if case.name.ends_with("matrix-6") {
            assert_eq!(truth.reachable_arms.last(), Some(&false));
        }
    }
    if count == 96 {
        assert_eq!(counts, [56, 24, 16]);
    }
    eprintln!(
        "match_coverage_22: {count} matrices, {} source/V accepts, {} missing coverage, {} dead-alternative rejects",
        counts[0], counts[1], counts[2]
    );
}

#[test]
fn match_coverage_22_enclosing_alternatives_claim_values_first() {
    // The first outer alternative claims false before the nested Or is tried.
    // Keeping the enclosing choice alone would still miss this dead alternative.
    let source =
        "type Choice=A(bool)|B\nfn main(x:Choice)->u32{match x{A(false) | A(false | true) | B=>1}}";
    let mut engine = QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let file = engine
        .sources
        .add("nested-or-22.hir", source.into())
        .unwrap();
    engine
        .parse_file(
            file,
            &FileContext {
                package: path("main").0.package,
                modules: vec![],
            },
        )
        .unwrap();
    let ds = match engine.check_definition(&path("main")) {
        Err(QueryError::Diagnostics(ds)) => ds,
        other => panic!("expected a dead nested alternative: {other:?}"),
    };
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert!(
        ds[0]
            .message
            .starts_with("pattern alternative can never match")
    );
    assert_eq!(engine.sources.slice(ds[0].primary).unwrap(), "false");
}
