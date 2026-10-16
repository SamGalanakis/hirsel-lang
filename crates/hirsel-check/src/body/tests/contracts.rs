use super::support;

#[test]
fn bodyless_signature_contracts_are_still_checked() {
    let (diagnostics, _, _) = support::check_source("fn fragment(x:u8) -> u8 requires x");
    assert_eq!(diagnostics[0].rule.as_str(), "FACT-1");
}
