//! LANG-8 payload-directed injection: plain `?` selects the arm `? Arm` names.
use super::*;

const TYPES: &str = "type ParseError = Empty()\ntype ConfigError = Missing()\n";

/// Each output's try plans, in source order.
fn tries(outputs: &[Output]) -> Vec<&TryPlan> {
    outputs.iter().flat_map(|o| &o.plans.tries).collect()
}

#[test]
fn plain_question_mark_plans_exactly_the_named_arm() {
    let source = format!(
        "{TYPES}type AppError = Limit(u64) | Input(ParseError) | Config(ConfigError)\n\
         fn parse() -> Result[u64, ParseError] {{ Error(Empty()) }}\n\
         fn named() -> Result[u64, AppError] {{ let n = parse() ? Input\nOk(n) }}\n\
         fn plain() -> Result[u64, AppError] {{ let n = parse()?\nOk(n) }}"
    );
    let (ds, outputs, _) = support::check_source(&source);
    assert!(ds.is_empty(), "{ds:?}");
    let [named, plain] = tries(&outputs)[..] else {
        panic!("two try plans")
    };
    let Some(CallTarget::Constructor(ContextualName::Variant { variant, .. })) = &plain.mapper
    else {
        panic!("plain ? injects through a constructor: {plain:?}")
    };
    assert_eq!(variant.0, 1);
    assert_eq!(named.mapper, plain.mapper);
    assert_eq!(named.mapper_passing, plain.mapper_passing);
    assert_eq!(
        (named.success_variant, named.error_variant),
        (plain.success_variant, plain.error_variant)
    );
    assert_eq!(
        named.propagated_error_variant,
        plain.propagated_error_variant
    );
}

#[test]
fn identity_comes_before_injection() {
    let (ds, outputs, _) = support::check_source(
        "type E = Wrap(box E) | Leaf()\n\
         fn leaf() -> Result[u64, E] { Error(Leaf()) }\n\
         fn outer() -> Result[u64, E] { let n = leaf()?\nOk(n) }",
    );
    assert!(ds.is_empty(), "{ds:?}");
    assert_eq!(tries(&outputs)[0].mapper, None);
}

#[test]
fn no_arm_offers_three_repairs_and_names_the_arm_to_add() {
    let source = format!(
        "{TYPES}type AppError = Input(ParseError)\n\
         fn config() -> Result[u64, ConfigError] {{ Error(Missing()) }}\n\
         fn run() -> Result[u64, AppError] {{ let n = config()?\nOk(n) }}"
    );
    let (ds, outputs, map) = support::check_source(&source);
    let [d] = &ds[..] else { panic!("{ds:?}") };
    assert_eq!(d.rule.as_str(), "LANG-8");
    assert_eq!(map.slice(d.primary), Some("?"));
    assert_eq!(
        d.message,
        "plain ? has no arm for `ConfigError` in `AppError`"
    );
    assert_eq!(map.slice(d.secondary[0].span), Some("config()"));
    let helps: Vec<_> = d.notes.iter().filter(|n| n.starts_with("help: ")).collect();
    assert_eq!(helps.len(), 3, "{:?}", d.notes);
    assert!(helps[0].starts_with("help: (a) add the arm `Config(ConfigError)` to `AppError`"));
    assert!(helps[0].contains("changes the error contract"));
    assert!(helps[1].starts_with("help: (b) handle the error at this site"));
    assert!(helps[2].starts_with("help: (c) map it explicitly with `? C`"));
    assert!(!d.notes.iter().any(|n| n.contains("STORE-7")));
    let fixes: Vec<_> = outputs.iter().flat_map(|o| &o.plans.arm_fixes).collect();
    let [fix] = &fixes[..] else {
        panic!("{fixes:?}")
    };
    assert_eq!(
        (fix.arm.as_str(), fix.target.as_str()),
        ("Config(ConfigError)", "AppError")
    );
    assert_eq!(fix.site, d.primary);
    assert!(!fix.serializable);
}

#[test]
fn serializable_error_notes_the_schema_change() {
    let source = "@serializable\ntype RateError = Unknown()\n\
         @serializable\ntype PriceError = Sku(u64)\n\
         fn lookup() -> Result[u64, RateError] { Error(Unknown()) }\n\
         fn price() -> Result[u64, PriceError] { let n = lookup()?\nOk(n) }";
    let (ds, outputs, _) = support::check_source(source);
    let [d] = &ds[..] else { panic!("{ds:?}") };
    assert!(
        d.notes.contains(
            &"`PriceError` is serializable: adding an arm changes its stored and wire schema (STORE-7, SER-2)"
                .to_string()
        ),
        "{:?}",
        d.notes
    );
    assert!(
        outputs
            .iter()
            .flat_map(|o| &o.plans.arm_fixes)
            .all(|f| f.serializable)
    );
}

#[test]
fn ambiguity_names_every_candidate_and_offers_no_edit() {
    let (ds, outputs, _) = support::check_source(
        "type StoreError = Corrupt()\ntype DbError = Open(StoreError) | Commit(StoreError)\n\
         fn open() -> Result[u64, StoreError] { Error(Corrupt()) }\n\
         fn save() -> Result[u64, DbError] { let n = open()?\nOk(n) }",
    );
    let [d] = &ds[..] else { panic!("{ds:?}") };
    assert_eq!(
        d.message,
        "plain ? is ambiguous: `StoreError` is the payload of 2 arms of `DbError`"
    );
    assert_eq!(
        d.notes,
        [
            "candidate arms: Open(StoreError), Commit(StoreError)",
            "help: name the arm, since each one is a different role: `? Open` or `? Commit`",
        ]
    );
    assert!(outputs.iter().all(|o| o.plans.arm_fixes.is_empty()));
}

#[test]
fn primitive_errors_are_never_injected() {
    let (ds, _, _) = support::check_source(
        "type CliError = Usage(String) | Code(u64)\n\
         fn flag() -> Result[u64, String] { Error(\"x\") }\n\
         fn run() -> Result[u64, CliError] { let n = flag()?\nOk(n) }",
    );
    let [d] = &ds[..] else { panic!("{ds:?}") };
    assert_eq!(
        d.message,
        "plain ? does not inject `String` errors: `String` is not a nominal error type"
    );
    assert_eq!(d.notes, ["help: name the arm that carries it: `? Usage`"]);
}

#[test]
fn proposed_arm_names_drop_a_trailing_error() {
    for (spelling, name) in [
        ("FileError", Some("File")),
        ("std@1/http.HttpError", Some("Http")),
        ("bufio.ReadError[E]", Some("Read")),
        ("Other", Some("Other")),
        ("Error", None),
        ("E", Some("E")),
    ] {
        assert_eq!(
            super::super::inject::arm_name(spelling).as_deref(),
            name,
            "{spelling}"
        );
    }
}
