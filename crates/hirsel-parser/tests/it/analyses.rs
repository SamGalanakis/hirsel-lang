//! PS-17: analysis annotations parse at every site they may stand, beside the
//! tree, and are LANG-7 anywhere else.
use crate::support;

#[test]
fn annotations_parse_beside_the_tree_at_every_site() {
    let (output, _) = support::parse(
        "@analysis(acme.units: result = m/s)\n@noalloc\n\
         fn f(@analysis(unit: m) a: u64, b: u64, @analysis(x) @analysis(y: [1, (2)]) c: u64) {}\n\
         type R {\n  @analysis(unit: m) x: u64,\n  y: u64,\n}\n\
         type V = A(u64, @analysis(unit: s) u8) | B\n\
         distinct type D(@analysis(unit: m) v: u64)\n",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let tree = &output.syntax.tree;
    let written: Vec<_> = tree
        .analyses
        .iter()
        .map(|a| {
            format!(
                "{} {:?} {} [{}] {:?}",
                a.item.0,
                a.site,
                a.name,
                a.payload.join(" "),
                a.spaced
            )
        })
        .collect();
    assert_eq!(
        written,
        [
            "0 Item acme.units [result = m / s] [false, true, true, false, false]",
            "0 Parameter(0) unit [m] [false]",
            "0 Parameter(2) x [] []",
            "0 Parameter(2) y [[ 1 , ( 2 ) ]] [false, false, false, true, false, false, false]",
            "1 Field(0) unit [m] [false]",
            "2 Payload { variant: 0, field: 1 } unit [s] [false]",
            "3 Field(0) unit [m] [false]",
        ]
    );
    // The item keeps its own attributes and name; nothing of the annotation enters it.
    let item = &tree.items[0];
    assert_eq!(item.attributes.len(), 1);
    assert_eq!(item.name.as_str(), "f");
}

#[test]
fn an_item_begins_after_its_leading_annotations() {
    let (output, _) =
        support::parse("@analysis(taint) fn f() {}\n@noalloc @analysis(taint) fn g() {}\n");
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let starts: Vec<_> = output
        .syntax
        .tree
        .items
        .iter()
        .map(|item| item.origin.span.start)
        .collect();
    // A trailing annotation stays inside the item that an attribute begins.
    assert_eq!(starts, [17, 27]);
}

#[test]
fn annotations_elsewhere_or_malformed_are_refused() {
    for (source, message) in [
        (
            "fn f(g: fn(@analysis(a) x: u64) -> u64) {}",
            "attaches only",
        ),
        (
            "fn f() { let g = fn(@analysis(a) x: u64) -> u64 { x } }",
            "attaches only",
        ),
        (
            "fn f() {\n  fn g(@analysis(a) x: u64) {}\n}",
            "attaches only",
        ),
        ("fn f(@analysis(a: (]) x: u64) {}", "must balance"),
        ("fn f(@analysis(a:) x: u64) {}", "cannot be empty"),
        ("fn f(@analysis(a: x) {}", "expected"),
    ] {
        let (output, _) = support::parse(source);
        let first = output
            .diagnostics
            .first()
            .unwrap_or_else(|| panic!("{source}"));
        assert!(
            first.message.contains(message),
            "{source}: {}",
            first.message
        );
    }
    let (output, _) = support::parse("fn f(@analysis(a) x: u64) {}");
    assert_eq!(output.diagnostics.first().map(|d| d.rule.as_str()), None);
    let (output, _) = support::parse("fn f() { let g = fn(@analysis(a) x: u64) -> u64 { x } }");
    assert_eq!(output.diagnostics[0].rule.as_str(), "LANG-7");
}
