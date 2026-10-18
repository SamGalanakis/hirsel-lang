//! SER-1 source twins exercise the complete pipeline, including independent V.
use hirsel_driver::compiler_adapters;
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};

fn check(source: &str, expected: Option<&str>) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("schema.hir", source.into()).unwrap();
        let result = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(
            result.diagnostics.first().map(|d| d.rule.as_str()),
            expected,
            "{source}\n{:?}",
            result.diagnostics
        );
        if expected.is_none() {
            assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        }
    });
}

#[test]
fn every_ineligible_leaf_has_a_data_twin() {
    for leaf in [
        "Dir",
        "Path",
        "Instant",
        "File",
        "Arena",
        "MonotonicClock",
        "WallClock",
        "Unsafe",
        "StrPos",
        "fn() -> u64 uses {}",
        "Ptr[u64]",
        "Shared[u64]",
        "Slots[u64]",
        "SlotId[u64]",
        "Mutex[u64]",
        "Iter[u64]",
        "SortedMap[u64,u64]",
        "TypeShape[u64]",
    ] {
        check(
            &format!("@serializable type Row {{ value: List[({leaf},u64)] }}"),
            Some("SER-1"),
        );
        check("@serializable type Row { value: List[(u64,u64)] }", None);
    }
    check(
        "resource type Token { n:u64 }\n@serializable type Row { value: Token }",
        Some("SER-1"),
    );
    check(
        "type Token { n:u64 }\n@serializable type Row { value: Token }",
        Some("SER-1"),
    );
    check(
        "@serializable type Token { n:u64 }\n@serializable type Row { value: Token }",
        None,
    );
    check("@serializable resource type Token { n:u64 }", Some("SER-1"));
    check(
        "@serializable @capability(global) pub opaque type Seal: plain uses {} {}",
        Some("SER-1"),
    );
}

#[test]
fn serialization_attribute_has_no_arguments_and_only_applies_to_types() {
    check("@serializable(1) type Row {n:u64}", Some("LANG-1"));
    check("@serializable type Row {n:u64}", None);
    check("@serializable fn f() {}", Some("LANG-7"));
    check("fn f() {}", None);
    check(
        "@serializable @serializable type Row {n:u64}",
        Some("LANG-6"),
    );
}

#[test]
fn growing_recursive_schemas_report_operational_exhaustion() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add(
                "growing.hir",
                "@serializable type Growing[T:serializable] = Step(List[Growing[List[T]]])".into(),
            )
            .unwrap();
        let result = check_progress(&mut engine, file, &default_context());
        assert!(
            matches!(result,Err(hirsel_types::QueryError::Operational(ref failure)) if failure.operation=="type-proof-tooling-limit"),
            "{result:?}"
        );
    });
}

#[test]
fn every_admitted_builtin_has_a_source_schema_and_nested_checks() {
    for schema in [
        "Array[u64]",
        "List[String]",
        "Inline[u64,4]",
        "Simd[u64,4]",
        "Packed[u64]",
        "Map[u64,String]",
        "Set[u64]",
        "box u64",
        "(u64,Bytes)",
        "Option[u64]",
        "Result[u64,String]",
    ] {
        check(&format!("@serializable type Row {{value:{schema}}}"), None);
    }
}
