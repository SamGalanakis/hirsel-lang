//! Eight causal ownership diagnostics, rendered by the whole pipeline. Each
//! was once one generic sentence; each now states what is provided and why,
//! what is required and why, and the repairs the checker accepts. Rules and
//! primary spans are unchanged. Folding (docs/spec/diagnostics.md)
//! joins diagnostics with the same provided cause at the same site, across
//! requirements; the folding cases are unit tests in hirsel-driver.
use std::path::Path;

fn rendered(path: &str) -> String {
    // Relative to this crate, so rendered paths are stable.
    let path = Path::new("../../conformance").join(path);
    let source = std::fs::read_to_string(&path).unwrap();
    let expected = source
        .lines()
        .find_map(|line| line.strip_prefix("// expect: reject "))
        .expect("snapshot fixture must expect rejection");
    let (rule, line) = expected.split_once(" at line ").unwrap();
    let output = hirsel_driver::check_path(&path).expect("checked");
    let first = output.diagnostics.first().expect("expected rejection");
    assert_eq!(first.rule.as_str(), rule);
    assert_eq!(
        output.sources.location(first.primary).unwrap().line,
        line.parse::<u32>().unwrap()
    );
    output
        .diagnostics
        .iter()
        .map(|d| d.render_human(&output.sources))
        .collect()
}

/// before: error[MODE-1]: a local value cannot be stored into global storage
#[test]
fn case_1_borrowed_parameter_into_mut_storage() {
    insta::assert_snapshot!(rendered("MODE/MODE-1-mut-storage-list-constructor-bad.hir"));
}
/// before: error[MODE-1]: this operation requires a global value
#[test]
fn case_2_mut_parameter_as_global_result() {
    insta::assert_snapshot!(rendered("MODE/MODE-1-mut-result-bad.hir"));
}
/// before: two MODE-1 errors (lines 9 and 10) naming no cause. They fail
/// different required causes (the binding `owned`, the global result), and
/// fold because each names `src` at its declaration as its provided cause.
#[test]
fn case_3_global_store_and_global_result() {
    insta::assert_snapshot!(rendered("MODE/MODE-1-global-store-bad.hir"));
}
/// before: error[PROV-1]: the value belongs to a different origin
#[test]
fn case_4_task_of_another_scope() {
    insta::assert_snapshot!(rendered("PROV/PROV-3-task-bad.hir"));
}
/// before: error[PROV-2]: a union or erased origin cannot satisfy a branded use
#[test]
fn case_5_erased_brand_into_branded_binding() {
    insta::assert_snapshot!(rendered("PROV/PROV-2-unary-assignment-bad.hir"));
}
/// before: error[PROV-4]: the value's placement does not outlive the required arena
#[test]
fn case_6_parameter_in_the_wrong_arena() {
    insta::assert_snapshot!(rendered("PROV/PROV-4-arena-aggregate-wrong-field-bad.hir"));
}
/// before: error[CLO-4]: escaping captures need immutable owned global values
#[test]
fn case_7_escaping_capture_of_a_borrowed_parameter() {
    insta::assert_snapshot!(rendered("CLO/CLO-4-stored-borrowed-capture-bad.hir"));
}
/// before: two MODE-1 errors at 15:3 naming no cause. Both name the closure
/// temporary; one fails the result relation, the other the temporary's scope.
#[test]
fn case_8_result_of_a_temporary_closure_environment() {
    insta::assert_snapshot!(rendered("CLO/CLO-1-captured-local-field-content-bad.hir"));
}

/// Every specialized capture/payload reject has a clean accepted twin. This
/// checks both explanation sides through production selection, under the rule
/// each corpus file expects.
#[test]
fn concurrency_boundaries_name_both_causes_and_keep_their_twins() {
    for stem in [
        "CONC-5-create-unique-payload",
        "CONC-5-payload-unique",
        "CONC-5-result-sendable",
        "CONC-5-taken-body-capture",
        "CONC-5-io-in-body",
        "CONC-5-channel-in-body",
        "CONC-5-nested-lock-in-body",
        "CONC-6-open-receiver-payload",
    ] {
        let path = Path::new("../../conformance/CONC");
        let bad = path.join(format!("{stem}-bad.hir"));
        let rejected = hirsel_driver::check_path(&bad).unwrap();
        let first = &rejected.diagnostics[0];
        let text = std::fs::read_to_string(&bad).unwrap();
        let expected = text
            .lines()
            .find_map(|line| line.strip_prefix("// expect: reject "))
            .and_then(|rest| rest.split(' ').next())
            .unwrap();
        assert_eq!(first.rule.as_str(), expected, "{stem}: {first:?}");
        assert!(
            first
                .secondary
                .iter()
                .any(|l| l.message.starts_with("provided: ")),
            "{stem}: {first:?}"
        );
        assert!(
            first
                .secondary
                .iter()
                .any(|l| l.message.starts_with("required: ")),
            "{stem}: {first:?}"
        );
        assert!(first.message.contains(", but "), "{stem}: {first:?}");
        let accepted = hirsel_driver::check_path(&path.join(format!("{stem}-ok.hir"))).unwrap();
        assert!(
            accepted.diagnostics.is_empty(),
            "{stem}: {:?}",
            accepted.diagnostics
        );
    }
}
/// #79: a constructed record placed by its part names that part and its
/// declaration as the provided cause, not the enclosing block.
#[test]
fn constructed_record_placed_by_its_part() {
    insta::assert_snapshot!(rendered("PROV/PROV-4-constructed-part-relation-bad.hir"));
}

#[test]
fn for_item_brand_diagnostic_names_the_declared_owner_without_internal_markers() {
    hirsel_engine::with_compiler_stack(|| {
        let text = rendered("PROV/PROV-1-for-item-find-reject.hir");
        assert!(!text.contains('\u{1}'), "{text}");
        assert!(text.contains("of other"), "{text}");
        let output = hirsel_driver::check_path(Path::new(
            "../../conformance/PROV/PROV-1-for-item-find-run.hir",
        ))
        .unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    });
}
