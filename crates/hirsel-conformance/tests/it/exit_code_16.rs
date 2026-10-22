use hirsel_conformance::{Actual, Compiler, Driver, Expect, Status, extract_rules, parse_header};
use std::path::Path;

#[test]
fn exit_code_16_refusal_twins_require_a_real_entry_admission_failure() {
    let rules = extract_rules("### PROG-3: Entry\n").unwrap();
    let text = "// rule: PROG-3\n// expect: refuse entry\n// twin: PROG/PROG-3-exit-code-run.hir\npub fn main() -> u8 { 2u8 }\n";
    let header = parse_header(
        Path::new("conformance/PROG/PROG-3-byte-bad.hir"),
        text,
        &rules,
    )
    .unwrap();
    assert_eq!(header.expect, Expect::RefuseEntry);
    assert!(header.expect.matches(&Actual::RefusedEntry));
    assert!(!header.expect.matches(&Actual::Accepted));
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let legal_entry = root.join("conformance/PROG/PROG-3-exit-code-run.hir");
    let (status, _) = Compiler.evaluate(&legal_entry, &Expect::RefuseEntry);
    assert!(
        matches!(status, Status::Fail(_)),
        "a valid entry must not satisfy a refusal oracle"
    );
}
